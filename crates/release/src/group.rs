//! The `Release` group's fold and its rules (`design/DESIGN.md`, Shape, "Release group"; "Install operations", Keys,
//! Site day, and Site loss). Pure: `decide` reads the fold, the anchor, and the time of the entry it would go in,
//! which the leader stamps and a follower's `verify` reads from the entry, so both judge the same instant.

use std::collections::{BTreeMap, BTreeSet};

use ed25519_dalek::VerifyingKey;
use graftyard_types::{Digest, ReleaseId, SiteId, Timestamp};
use sha2::{Digest as _, Sha256};

use crate::types::{
    DayTarget, Format, FormatSet, Genesis, Intent, IntentId, IntentState, KeyId, Keys, Mode, ReleaseEntry,
    ReleaseUploaded, Scope, Script, SecretName, Signature, SignedIntent, VersionId,
};
use crate::{Anchor, Bound, Refusal, SignedEntry, check_genesis, check_keys};

/// How long a drain-key or intent-key drain must have held, with zero acks read by `graft-ops`, before the executor
/// key may replace the site ("Install operations", Site day, `ZERO_ACKS`) [guess].
pub const ZERO_ACKS: u64 = 24 * 3_600_000;
/// The longest an uncommitted `StaleMode` is honored after it arrives (Shape, "The read path") [guess].
pub const STALE_MAX: u64 = 24 * 3_600_000;
/// The secrets the agents account's `graft-ops` rotates, whose `Rotate` the agents executor key may step.
const AGENTS_SECRETS: [SecretName; 2] = [SecretName::AgentsAppKey, SecretName::WorkspaceAppKey];

/// The bytes the release-signing half of `FormatsEnable` signs: the format alone, so the release record can carry it.
pub fn formats_enable_message(f: Format) -> Vec<u8> {
    let mut m = b"graftyard/formats-enable/1".to_vec();
    m.extend_from_slice(&f.family.to_be_bytes());
    m.extend_from_slice(&f.version.to_be_bytes());
    m
}

/// A release's upload, as the fold keeps it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Upload {
    pub versions: Vec<(Script, VersionId)>,
    pub reads: FormatSet,
}

/// A site's drain: which key class set it (a drain-key drain ends only by a drain-key `Normal`, an intent-key one only
/// by an intent-key `Normal`) and the time of the entry that set it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Drained {
    pub by_drain_key: bool,
    pub since: Timestamp,
}

/// Each pin: `None` follows the deployment split, `Some` is set only by a pre-signed pointer undo.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pointers {
    pub edge: Option<VersionId>,
    pub ops: Option<VersionId>,
    pub mcp: Option<VersionId>,
    pub controller: Option<VersionId>,
}

impl Pointers {
    fn slot(&mut self, s: Script) -> Option<&mut Option<VersionId>> {
        match s {
            Script::Edge => Some(&mut self.edge),
            Script::Ops => Some(&mut self.ops),
            Script::Mcp => Some(&mut self.mcp),
            Script::Controller => Some(&mut self.controller),
            _ => None,
        }
    }
}

/// The current intent: its signed body, where it is, and the undo list that is live for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Current {
    pub intent: SignedIntent,
    pub state: IntentState,
    /// The next step `IntentStep` must name. Restarts at 0 when an undo or `Abort` commits.
    pub next: u8,
    /// The live undo list: the intent's own, or the last `Resume`'s non-empty one.
    pub undo: Vec<(ReleaseEntry, Signature)>,
    /// The key the live list was checked against when it committed, with the `Keys` current then.
    pub undo_key: KeyId,
    /// Undos already committed, which a replay must not commit again.
    pub used: Vec<ReleaseEntry>,
    /// Whether the vendor key signed the intent.
    pub vendor_signed: bool,
}

/// What the `Release` group's fold holds. `Noop` and `Config` entries never reach it, so `last` is the digest of the
/// last committed signed entry, which the next one binds as `prev`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReleaseFold {
    pub genesis: Option<Genesis>,
    pub genesis_digest: Option<Digest>,
    pub keys: Option<Keys>,
    pub last: Option<Digest>,
    /// Signed entries applied, genesis included.
    pub count: u64,
    pub current: Option<Box<Current>>,
    /// The intent a `ReplaceMember` halted, current again once the replacement is done.
    pub under: Option<Box<Current>>,
    pub seen: BTreeSet<IntentId>,
    pub uploads: BTreeMap<[u8; 32], Upload>,
    pub enabled: BTreeSet<Format>,
    pub drained: BTreeMap<u8, Drained>,
    pub lost: BTreeSet<u8>,
    pub site_release: BTreeMap<u8, ReleaseId>,
    pub pointers: Pointers,
    /// Each pinned script's deployment as the rolls left it: the release kept (N) and the one rolled (N+1).
    pub deployed: BTreeMap<Script, (Option<ReleaseId>, ReleaseId)>,
    pub lifeboats: BTreeMap<Box<str>, KeyId>,
    pub stale: Vec<(Scope, Timestamp)>,
}

/// The one event an accepted entry commits: the entry's canonical stored bytes, so every member applies and digests
/// the same bytes. `Record::encode` for it is `bytes` itself.
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

fn refuse(why: impl Into<String>) -> Refusal {
    Refusal::Rejected(why.into().into_boxed_str())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    Customer,
    Vendor,
    Release,
    Drain,
    Executor,
    AgentsExecutor,
}

fn role(k: &Keys, key: &KeyId) -> Option<Role> {
    if *key == k.customer {
        Some(Role::Customer)
    } else if Some(*key) == k.vendor {
        Some(Role::Vendor)
    } else if *key == k.release {
        Some(Role::Release)
    } else if *key == k.drain {
        Some(Role::Drain)
    } else if *key == k.executor {
        Some(Role::Executor)
    } else if Some(*key) == k.agents_executor {
        Some(Role::AgentsExecutor)
    } else {
        None
    }
}

fn verify_raw(key: &KeyId, msg: &[u8], sig: &Signature) -> Result<(), Refusal> {
    let vk = VerifyingKey::from_bytes(&key.0).map_err(|_| Refusal::BadSignature)?;
    vk.verify_strict(msg, &ed25519_dalek::Signature::from_bytes(&sig.0)).map_err(|_| Refusal::BadSignature)
}

/// The signature of the key whose seed is `seed` over `undo` pre-signed for intent `intent` of install `install`: what
/// `SignedIntent.undo` and `Resume.undo` carry next to each undo.
pub fn presign_undo(install: crate::InstallId, intent: IntentId, undo: &ReleaseEntry, seed: &[u8; 32]) -> Signature {
    let mut e = SignedEntry::unsigned(undo_bound(install, intent), undo.clone());
    e.sign(seed);
    e.sigs[0].1
}

/// The entry an edge or `graft-ops` submits for a pre-signed undo: bound to its parent, with the list's signature.
pub fn undo_entry(
    install: crate::InstallId,
    intent: IntentId,
    undo: &ReleaseEntry,
    key: KeyId,
    sig: Signature,
) -> SignedEntry {
    let mut e = SignedEntry::unsigned(undo_bound(install, intent), undo.clone());
    e.sigs.push((key, sig));
    e
}

/// The bound a pre-signed undo of intent `id` is signed over.
pub(crate) fn undo_bound(install: crate::InstallId, id: IntentId) -> Bound {
    Bound { install, group: graftyard_types::ChainId::Release, prev: None, parent: Some(id) }
}

impl ReleaseFold {
    fn keys(&self) -> &Keys {
        self.keys.as_ref().expect("an installed group holds Keys")
    }

    fn upload(&self, r: &ReleaseId) -> Result<&Upload, Refusal> {
        self.uploads.get(&r.0).ok_or(Refusal::NoUpload(*r))
    }

    /// The release has a `ReleaseUploaded` and reads every enabled format.
    fn readable(&self, r: &ReleaseId) -> Result<&Upload, Refusal> {
        let u = self.upload(r)?;
        match self.enabled.iter().find(|f| !u.reads.contains(f)) {
            Some(f) => Err(Refusal::UnreadableFormat { release: *r, format: *f }),
            None => Ok(u),
        }
    }

    fn known_site(&self, s: SiteId) -> Result<&graftyard_types::Site, Refusal> {
        let g = self.genesis.as_ref().expect("installed");
        g.sites.iter().find(|x| x.id == s).ok_or_else(|| refuse(format!("site {} is not in the install", s.0)))
    }

    /// The intent, current or halted under a `ReplaceMember`, that `id` names.
    fn intent_of(&self, id: IntentId) -> Option<&Current> {
        [&self.current, &self.under].into_iter().flatten().map(|c| &**c).find(|c| c.intent.id == id)
    }

    /// `decide` for a submitted entry, in an entry stamped `at`. Pure.
    pub fn decide(&self, anchor: &Anchor, at: Timestamp, bytes: &[u8]) -> Result<Accepted, Refusal> {
        let entry = SignedEntry::decode(bytes).map_err(Refusal::Malformed)?;
        let Some(genesis) = &self.genesis else {
            check_genesis(anchor, &entry)?;
            return Ok(Accepted { bytes: entry.encode().into_boxed_slice() });
        };
        let b = &entry.bound;
        if b.group != graftyard_types::ChainId::Release {
            return Err(Refusal::WrongGroup(b.group));
        }
        if b.install != genesis.install {
            return Err(Refusal::WrongInstall { want: genesis.install, got: b.install });
        }
        if let ReleaseEntry::Genesis(_) = entry.body {
            return Err(Refusal::AlreadyInstalled(self.genesis_digest.expect("installed")));
        }
        if entry.body.undo_only() {
            self.check_undo(&entry)?;
        } else {
            let last = self.last.expect("installed");
            if b.prev != Some(last) {
                return Err(Refusal::StalePrev(last));
            }
            if b.parent.is_some() {
                return Err(Refusal::BadBound("only a pre-signed undo binds a parent intent"));
            }
            self.check_signers(&entry)?;
            self.check_body(&entry, at)?;
        }
        Ok(Accepted { bytes: entry.encode().into_boxed_slice() })
    }

    /// A follower's check of the event before it applies it: the same rules, on the fold it holds.
    pub fn check(&self, anchor: &Anchor, at: Timestamp, ev: &Accepted) -> Result<(), Refusal> {
        self.decide(anchor, at, &ev.bytes).map(|_| ())
    }

    /// Every signature is by a key of the latest `Keys` in a role that may sign this kind, and verifies.
    fn check_signers(&self, e: &SignedEntry) -> Result<(), Refusal> {
        let k = self.keys();
        let msg = e.signed_bytes();
        let need = |allowed: &[Role]| -> Result<(), Refusal> {
            let [(key, sig)] = e.sigs.as_slice() else {
                return Err(Refusal::WrongSigner(e.body.kind()));
            };
            match role(k, key) {
                None => Err(Refusal::UnknownKey(*key)),
                Some(r) if !allowed.contains(&r) => Err(Refusal::WrongSigner(e.body.kind())),
                Some(_) => verify_raw(key, &msg, sig),
            }
        };
        use Role::*;
        match &e.body {
            ReleaseEntry::Keys(new) => {
                // The current customer key, and the new one as well when it changes; no other signature.
                let mut want = vec![k.customer];
                if new.customer != k.customer {
                    want.push(new.customer);
                }
                for (key, _) in &e.sigs {
                    if !want.contains(key) {
                        return Err(if role(k, key).is_none() {
                            Refusal::UnknownKey(*key)
                        } else {
                            Refusal::WrongSigner("Keys")
                        });
                    }
                }
                if e.sigs.len() != want.len() {
                    return Err(Refusal::WrongSigner("Keys"));
                }
                for (key, sig) in &e.sigs {
                    verify_raw(key, &msg, sig)?;
                }
                Ok(())
            }
            ReleaseEntry::FormatsEnable { format } => {
                // The customer key over the entry, plus the release-signing key's pre-signed half over the format.
                let customer = e.sigs.iter().find(|(key, _)| *key == k.customer);
                let release = e.sigs.iter().find(|(key, _)| *key == k.release);
                if let Some((key, _)) = e.sigs.iter().find(|(key, _)| role(k, key).is_none()) {
                    return Err(Refusal::UnknownKey(*key));
                }
                let (Some(c), Some(r)) = (customer, release) else { return Err(Refusal::WrongSigner("FormatsEnable")) };
                if e.sigs.len() != 2 {
                    return Err(Refusal::WrongSigner("FormatsEnable"));
                }
                verify_raw(&c.0, &msg, &c.1)?;
                verify_raw(&r.0, &formats_enable_message(*format), &r.1)
            }
            ReleaseEntry::Intent(i) => match i.intent {
                Intent::ReplaceMember { .. } => need(&[Executor]),
                Intent::LeaveEvaluation(_) => need(&[Customer]),
                _ => need(&[Vendor, Customer]),
            },
            ReleaseEntry::SiteMode(_) => need(&[Vendor, Customer, Drain]),
            ReleaseEntry::ReleaseUploaded(u) => {
                let agents = !u.versions.is_empty()
                    && u.versions.iter().all(|(s, _)| matches!(s, Script::AgentsFront | Script::Controller));
                if agents { need(&[Executor, AgentsExecutor]) } else { need(&[Executor]) }
            }
            ReleaseEntry::IntentStep { intent, .. } | ReleaseEntry::IntentDone { intent } => {
                let agents = self.intent_of(*intent).is_some_and(|c| match &c.intent.intent {
                    Intent::ControllerRoll { .. } => true,
                    Intent::Rotate { secret, .. } => AGENTS_SECRETS.contains(secret),
                    _ => false,
                });
                if agents { need(&[Executor, AgentsExecutor]) } else { need(&[Executor]) }
            }
            ReleaseEntry::SiteRelease { .. } => need(&[Executor]),
            ReleaseEntry::LifeboatDecoder { host, .. } => {
                let key =
                    self.lifeboats.get(host).ok_or_else(|| refuse(format!("no lifeboat {host} is registered")))?;
                let [(signer, sig)] = e.sigs.as_slice() else { return Err(Refusal::WrongSigner("LifeboatDecoder")) };
                if signer != key {
                    return Err(if role(k, signer).is_none() {
                        Refusal::UnknownKey(*signer)
                    } else {
                        Refusal::WrongSigner("LifeboatDecoder")
                    });
                }
                verify_raw(signer, &msg, sig)
            }
            ReleaseEntry::SiteLost { .. }
            | ReleaseEntry::StaleMode { .. }
            | ReleaseEntry::LifeboatAdd { .. }
            | ReleaseEntry::LifeboatRemove { .. }
            | ReleaseEntry::RepoRestore { .. }
            | ReleaseEntry::RepoReset { .. }
            | ReleaseEntry::Halt { .. }
            | ReleaseEntry::Resume { .. }
            | ReleaseEntry::Abort { .. } => need(&[Customer]),
            ReleaseEntry::Genesis(_)
            | ReleaseEntry::Rollback { .. }
            | ReleaseEntry::EdgePointer { .. }
            | ReleaseEntry::OpsPointer { .. }
            | ReleaseEntry::McpPointer { .. }
            | ReleaseEntry::ControllerPointer { .. } => unreachable!("handled before"),
        }
    }

    /// A pre-signed undo: bound to a parent intent that is current and not aborting, byte for byte an entry of its
    /// live list with that list's signature, not committed before, and naming a release every enabled format reads.
    fn check_undo(&self, e: &SignedEntry) -> Result<(), Refusal> {
        if e.bound.prev.is_some() {
            return Err(Refusal::BadBound("a pre-signed undo binds its parent intent, not prev"));
        }
        let Some(parent) = e.bound.parent else {
            return Err(Refusal::NotAnUndo(e.body.kind()));
        };
        let c = self.intent_of(parent).ok_or(Refusal::NotCurrent(parent))?;
        if let IntentState::Aborting { .. } = c.state {
            return Err(Refusal::BadState("an aborted intent's undos are refused"));
        }
        let [(key, sig)] = e.sigs.as_slice() else { return Err(Refusal::NotAnUndo(e.body.kind())) };
        if !c.undo.iter().any(|(u, s)| *u == e.body && s == sig) || *key != c.undo_key {
            return Err(Refusal::NotAnUndo(e.body.kind()));
        }
        if c.used.contains(&e.body) {
            return Err(Refusal::UndoUsed(e.body.kind()));
        }
        verify_raw(key, &e.signed_bytes(), sig)?;
        self.check_undo_body(&e.body)
    }

    fn check_undo_body(&self, u: &ReleaseEntry) -> Result<(), Refusal> {
        match u {
            ReleaseEntry::Rollback { target, release } => {
                if let DayTarget::Site(s) | DayTarget::Containers(s) = target {
                    self.known_site(*s)?;
                }
                self.readable(release).map(|_| ())
            }
            _ => {
                let (script, pin) = u.pointer().ok_or(Refusal::NotAnUndo(u.kind()))?;
                let up = self.readable(&pin)?;
                if !up.versions.iter().any(|(s, _)| *s == script) {
                    return Err(refuse(format!("release {} uploaded no {script:?} version", hex32(&pin.0))));
                }
                Ok(())
            }
        }
    }

    /// The undo list an intent or `Resume` carries, signed by `key` for intent `id`.
    fn check_undo_list(&self, id: IntentId, key: &KeyId, undo: &[(ReleaseEntry, Signature)]) -> Result<(), Refusal> {
        let install = self.genesis.as_ref().expect("installed").install;
        for (u, sig) in undo {
            let e = SignedEntry::unsigned(undo_bound(install, id), u.clone());
            verify_raw(key, &e.signed_bytes(), sig)?;
            self.check_undo_body(u)?;
        }
        Ok(())
    }

    fn check_body(&self, e: &SignedEntry, at: Timestamp) -> Result<(), Refusal> {
        let signer = e.sigs.first().map(|(k, _)| *k);
        match &e.body {
            ReleaseEntry::Keys(new) => {
                let want = self.keys().generation + 1;
                if new.generation != want {
                    return Err(Refusal::Generation { want, got: new.generation });
                }
                check_keys(new).map_err(refuse)
            }
            ReleaseEntry::Intent(i) => self.check_intent(i, signer.expect("signed"), at),
            ReleaseEntry::SiteMode(m) => {
                self.known_site(m.site)?;
                let by_drain = signer == Some(self.keys().drain);
                let held = self.drained.get(&m.site.0);
                match (m.mode, held) {
                    (Mode::Drain, Some(_)) => Err(Refusal::BadState("the site is already drained")),
                    (Mode::Drain, None) => {
                        if by_drain && let Some(other) = self.drained.keys().next() {
                            return Err(Refusal::DrainGuard(SiteId(*other)));
                        }
                        Ok(())
                    }
                    (Mode::Normal, None) => Err(Refusal::BadState("the site is not drained")),
                    (Mode::Normal, Some(d)) if d.by_drain_key != by_drain => Err(Refusal::WrongSigner("SiteMode")),
                    (Mode::Normal, Some(_)) => Ok(()),
                }
            }
            ReleaseEntry::ReleaseUploaded(u) => self.check_upload(u),
            ReleaseEntry::IntentStep { intent, step } => {
                let c =
                    self.current.as_deref().filter(|c| c.intent.id == *intent).ok_or(Refusal::NotCurrent(*intent))?;
                if let IntentState::Halted { .. } = c.state {
                    return Err(Refusal::BadState("the intent is halted"));
                }
                if *step != c.next {
                    return Err(Refusal::WrongStep { want: c.next, got: *step });
                }
                Ok(())
            }
            ReleaseEntry::IntentDone { intent } => {
                let c =
                    self.current.as_deref().filter(|c| c.intent.id == *intent).ok_or(Refusal::NotCurrent(*intent))?;
                match c.state {
                    IntentState::Halted { .. } => Err(Refusal::BadState("the intent is halted")),
                    _ => Ok(()),
                }
            }
            ReleaseEntry::SiteRelease { site, release } => {
                self.known_site(*site)?;
                self.upload(release).map(|_| ())
            }
            ReleaseEntry::SiteLost { site } => {
                let s = self.known_site(*site)?;
                if s.spare {
                    return Err(refuse("a spare site holds no voter to lose"));
                }
                Ok(())
            }
            ReleaseEntry::StaleMode { until, .. } => {
                if *until <= at {
                    return Err(refuse("a StaleMode flag whose until has passed"));
                }
                Ok(())
            }
            ReleaseEntry::LifeboatAdd { host, key } => {
                if self.lifeboats.contains_key(host) {
                    return Err(refuse(format!("lifeboat {host} is already registered")));
                }
                if VerifyingKey::from_bytes(&key.0).is_err() || role(self.keys(), key).is_some() {
                    return Err(refuse("a lifeboat key is its own Ed25519 key"));
                }
                Ok(())
            }
            ReleaseEntry::LifeboatRemove { host } => {
                self.lifeboats.get(host).map(|_| ()).ok_or_else(|| refuse(format!("no lifeboat {host} is registered")))
            }
            ReleaseEntry::LifeboatDecoder { release, .. } => self.upload(release).map(|_| ()),
            ReleaseEntry::FormatsEnable { format } => {
                if self.enabled.contains(format) {
                    return Err(refuse(format!("format {format} is already enabled")));
                }
                Ok(())
            }
            ReleaseEntry::RepoRestore { repos, reason, .. } => {
                if repos.is_empty() || reason.is_empty() {
                    return Err(refuse("a restore names its repositories and a reason"));
                }
                Ok(())
            }
            ReleaseEntry::RepoReset { reason, .. } => {
                if reason.is_empty() {
                    return Err(refuse("a reset gives a reason"));
                }
                Ok(())
            }
            ReleaseEntry::Halt { intent } => {
                let c =
                    self.current.as_deref().filter(|c| c.intent.id == *intent).ok_or(Refusal::NotCurrent(*intent))?;
                match c.state {
                    IntentState::Running { .. } => Ok(()),
                    _ => Err(Refusal::BadState("only a running intent halts")),
                }
            }
            ReleaseEntry::Resume { intent, undo } => {
                let c =
                    self.current.as_deref().filter(|c| c.intent.id == *intent).ok_or(Refusal::NotCurrent(*intent))?;
                let IntentState::Halted { .. } = c.state else {
                    return Err(Refusal::BadState("only a halted intent resumes"));
                };
                // A vendor-signed intent whose key is gone resumes only on an undo list the customer re-signed.
                if c.vendor_signed && self.keys().vendor.is_none_or(|v| v != c.undo_key) && undo.is_empty() {
                    return Err(refuse("resuming a vendor intent after the vendor key left needs a re-signed undo"));
                }
                self.check_undo_list(*intent, &self.keys().customer, undo)
            }
            ReleaseEntry::Abort { intent, target } => {
                let c =
                    self.current.as_deref().filter(|c| c.intent.id == *intent).ok_or(Refusal::NotCurrent(*intent))?;
                if let IntentState::Aborting { .. } = c.state {
                    return Err(Refusal::BadState("the intent is already aborting"));
                }
                self.readable(target).map(|_| ())
            }
            ReleaseEntry::Genesis(_)
            | ReleaseEntry::Rollback { .. }
            | ReleaseEntry::EdgePointer { .. }
            | ReleaseEntry::OpsPointer { .. }
            | ReleaseEntry::McpPointer { .. }
            | ReleaseEntry::ControllerPointer { .. } => unreachable!("handled before"),
        }
    }

    fn check_upload(&self, u: &ReleaseUploaded) -> Result<(), Refusal> {
        if self.uploads.contains_key(&u.release.0) {
            return Err(refuse(format!("release {} is already uploaded", hex32(&u.release.0))));
        }
        if Sha256::digest(&u.record).as_slice() != u.release.0 {
            return Err(Refusal::BadRecord("the release id is not the SHA-256 of its record".into()));
        }
        verify_raw(&self.keys().release, &u.record, &u.record_sig)
            .map_err(|_| Refusal::BadRecord("the record's signature does not verify under Keys.release".into()))?;
        // `reads` is the bundle's formats file, whose SHA-256 the signed record lists.
        let want = hex32(&Sha256::digest(u.reads.file().as_bytes()).into());
        let rec: serde_json::Value = serde_json::from_slice(&u.record)
            .map_err(|_| Refusal::BadRecord("the record is not release.json".into()))?;
        let listed = rec["files"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|f| f["name"].as_str() == Some("formats.txt") && f["sha256"].as_str() == Some(want.as_str()));
        if !listed {
            return Err(Refusal::BadRecord("the record lists no formats.txt with these reads".into()));
        }
        let mut scripts: Vec<Script> = u.versions.iter().map(|(s, _)| *s).collect();
        scripts.sort_unstable();
        if scripts.is_empty() || scripts.windows(2).any(|w| w[0] == w[1]) {
            return Err(refuse("an upload names each script once"));
        }
        Ok(())
    }

    fn check_intent(&self, i: &SignedIntent, signer: KeyId, at: Timestamp) -> Result<(), Refusal> {
        if at > i.not_after {
            return Err(Refusal::Expired { not_after: i.not_after });
        }
        if i.issued_at > i.not_after {
            return Err(refuse("issued_at is after not_after"));
        }
        if self.seen.contains(&i.id) {
            return Err(refuse("this intent id was used before"));
        }
        if let Some(r) = i.intent.release() {
            self.readable(&r)?;
        }
        if let (Some(script), Some(r)) = (i.intent.pinned(), i.intent.release()) {
            // N, the release the script runs now, stays in the deployment next to R, and every other version leaves
            // it, so an undo of this intent that pins any other version could never apply ("A version override
            // will only be applied if the specified version is in the current deployment"). No other intent is
            // current while this one commits, so this intent's undos are the only live ones.
            let kept = self.deployed.get(&script).map(|(_, cur)| *cur);
            for (u, _) in &i.undo {
                if let Some((s, pin)) = u.pointer()
                    && s == script
                    && pin != r
                    && kept.is_some_and(|k| k != pin)
                {
                    return Err(Refusal::DropsUndoVersion(pin));
                }
            }
        }
        if let Some(c) = &self.current {
            let replace = matches!(i.intent, Intent::ReplaceMember { .. });
            if !replace || self.under.is_some() || matches!(c.intent.intent, Intent::ReplaceMember { .. }) {
                return Err(Refusal::IntentCurrent(c.intent.id));
            }
        }
        match &i.intent {
            Intent::SiteDay { target: DayTarget::Site(s) | DayTarget::Containers(s), .. }
            | Intent::ContainerDay { site: s, .. } => {
                self.known_site(*s)?;
            }
            Intent::ReplaceMember { lost, spare } => {
                if !i.undo.is_empty() {
                    return Err(refuse("a ReplaceMember carries no undo: the executor key pins nothing"));
                }
                // The loss first: it is the rule that keeps a leaked executor key off a healthy site.
                let zero_acks = self.drained.get(&lost.0).is_some_and(|d| d.since.0.saturating_add(ZERO_ACKS) <= at.0);
                if !self.lost.contains(&lost.0) && !zero_acks {
                    return Err(Refusal::NotLost(*lost));
                }
                if self.known_site(*lost)?.spare || !self.known_site(*spare)?.spare {
                    return Err(refuse("ReplaceMember moves a voter site's members onto a spare"));
                }
            }
            Intent::RestorePlacement { site } => {
                self.known_site(*site)?;
                if self.drained.contains_key(&site.0) {
                    return Err(refuse("RestorePlacement is refused while its site is drained"));
                }
            }
            Intent::Rotate { slot, .. } => {
                if *slot > 1 {
                    return Err(refuse("a secret has slots 0 and 1"));
                }
            }
            Intent::LeaveEvaluation(_) if !self.genesis.as_ref().expect("installed").evaluation => {
                return Err(refuse("LeaveEvaluation needs the evaluation flag"));
            }
            _ => {}
        }
        self.check_undo_list(i.id, &signer, &i.undo)
    }

    /// Applies an accepted entry committed in an entry stamped `at`.
    pub fn apply(&mut self, ev: &Accepted, at: Timestamp) {
        let entry = ev.entry();
        let digest = ev.digest();
        self.last = Some(digest);
        self.count += 1;
        let signer = entry.sigs.first().map(|(k, _)| *k);
        match entry.body {
            ReleaseEntry::Genesis(g) => {
                self.keys = Some(g.keys.clone());
                self.genesis = Some(*g);
                self.genesis_digest = Some(digest);
            }
            ReleaseEntry::Keys(k) => self.keys = Some(*k),
            ReleaseEntry::Intent(i) => {
                self.seen.insert(i.id);
                if let Some(s) = i.intent.pinned() {
                    // The next intent that rolls a pinned script clears its pin, and N stays beside R.
                    if let Some(slot) = self.pointers.slot(s) {
                        *slot = None;
                    }
                    let r = i.intent.release().expect("a roll names its release");
                    let kept = self.deployed.get(&s).map(|(_, cur)| *cur);
                    self.deployed.insert(s, (kept, r));
                }
                if let Intent::LeaveEvaluation(l) = &i.intent {
                    let (sites, placements) = (&l.sites, &l.placements);
                    let g = self.genesis.as_mut().expect("installed");
                    g.sites.extend(sites.iter().cloned());
                    for (c, new) in placements {
                        if let Some((_, r)) = g.placements.iter_mut().find(|(x, _)| x == c) {
                            let old = std::mem::replace(r, new.clone());
                            r.moving_from = Some(Box::new(old));
                        }
                    }
                    g.evaluation = false;
                }
                let key = signer.expect("signed");
                let vendor_signed = self.keys().vendor == Some(key);
                let c = Box::new(Current {
                    undo: i.undo.clone(),
                    undo_key: key,
                    state: IntentState::Running { step: 0 },
                    next: 0,
                    used: Vec::new(),
                    vendor_signed,
                    intent: *i,
                });
                if let Some(mut prev) = self.current.take() {
                    // Only a `ReplaceMember` commits over a current intent, and it halts it.
                    if let IntentState::Running { step } = prev.state {
                        prev.state = IntentState::Halted { step };
                    }
                    self.under = Some(prev);
                }
                self.current = Some(c);
            }
            ReleaseEntry::SiteMode(m) => match m.mode {
                Mode::Drain => {
                    let by_drain_key = signer == Some(self.keys().drain);
                    self.drained.insert(m.site.0, Drained { by_drain_key, since: at });
                }
                Mode::Normal => {
                    self.drained.remove(&m.site.0);
                }
            },
            ReleaseEntry::ReleaseUploaded(u) => {
                self.uploads.insert(u.release.0, Upload { versions: u.versions.to_vec(), reads: u.reads.clone() });
            }
            ReleaseEntry::IntentStep { .. } => {
                let c = self.current.as_mut().expect("checked");
                c.next += 1;
                if let IntentState::Running { step } = &mut c.state {
                    *step = c.next;
                }
            }
            ReleaseEntry::IntentDone { .. } => self.current = self.under.take(),
            ReleaseEntry::SiteRelease { site, release } => {
                self.site_release.insert(site.0, release);
            }
            ref u @ (ReleaseEntry::Rollback { .. }
            | ReleaseEntry::EdgePointer { .. }
            | ReleaseEntry::OpsPointer { .. }
            | ReleaseEntry::McpPointer { .. }
            | ReleaseEntry::ControllerPointer { .. }) => {
                if let Some((script, pin)) = u.pointer() {
                    let v = self.uploads[&pin.0].versions.iter().find(|(s, _)| *s == script).map(|(_, v)| *v);
                    if let Some(slot) = self.pointers.slot(script) {
                        *slot = v;
                    }
                }
                let parent = entry.bound.parent.expect("an undo binds its parent");
                let c = [&mut self.current, &mut self.under]
                    .into_iter()
                    .flatten()
                    .find(|c| c.intent.id == parent)
                    .expect("checked");
                c.used.push(u.clone());
                if !matches!(c.state, IntentState::Undoing) {
                    c.state = IntentState::Undoing;
                    c.next = 0;
                }
            }
            ReleaseEntry::SiteLost { site } => {
                self.lost.insert(site.0);
            }
            ReleaseEntry::StaleMode { scope, until } => self.stale.push((scope, until)),
            ReleaseEntry::LifeboatAdd { host, key } => {
                self.lifeboats.insert(host, key);
            }
            ReleaseEntry::LifeboatRemove { host } => {
                self.lifeboats.remove(&host);
            }
            ReleaseEntry::LifeboatDecoder { .. }
            | ReleaseEntry::RepoRestore { .. }
            | ReleaseEntry::RepoReset { .. } => {}
            ReleaseEntry::FormatsEnable { format } => {
                self.enabled.insert(format);
            }
            ReleaseEntry::Halt { .. } => {
                let c = self.current.as_mut().expect("checked");
                if let IntentState::Running { step } = c.state {
                    c.state = IntentState::Halted { step };
                }
            }
            ReleaseEntry::Resume { undo, .. } => {
                let customer = self.keys().customer;
                let c = self.current.as_mut().expect("checked");
                if let IntentState::Halted { step } = c.state {
                    c.state = IntentState::Running { step };
                }
                if !undo.is_empty() {
                    c.undo = undo;
                    c.undo_key = customer;
                }
            }
            ReleaseEntry::Abort { target, .. } => {
                let c = self.current.as_mut().expect("checked");
                c.state = IntentState::Aborting { target };
                c.next = 0;
            }
        }
    }
}

pub(crate) fn hex32(b: &[u8; 32]) -> String {
    crate::types::hex(b)
}
