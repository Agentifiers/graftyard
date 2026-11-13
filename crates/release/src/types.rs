//! The `Release` group's types (`design/DESIGN.md`, Shape, "Release group"), and their canonical bytes. Enum tags
//! follow the design's declaration order. The wire format is append-only: genesis (tag 0) and `Keys` (tag 1) were
//! built first and every install's log holds a genesis, so no tag moves and a new variant takes the next one.

use std::fmt;
use std::str::FromStr;

use graftyard_types::{
    AccountId, ChainId, DbId, Digest, Jurisdiction, PlacementName, PlacementRecord, Region, ReleaseId, RepoId, Site,
    SiteId, SiteLink, SystemChain, Timestamp, Ulid,
};
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

use crate::codec::{Dec, Enc, Malformed, bad};

/// Longest string field a decoder takes: script, account, bucket, class, and host names are far shorter.
const STR_MAX: usize = 1024;
/// Largest `release.json` a `ReleaseUploaded` carries [guess].
const RECORD_MAX: usize = 256 * 1024;

/// Minted by `graftyard init`. Its text form is the ULID's, which is what `GRAFTYARD_INSTALL` holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct InstallId(pub Ulid);

impl fmt::Display for InstallId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for InstallId {
    type Err = graftyard_types::ParseUlidError;
    fn from_str(s: &str) -> Result<InstallId, Self::Err> {
        s.parse().map(InstallId)
    }
}

/// An Ed25519 public key. The design names keys by `KeyId` and gives it no other form, so the key is its own id.
/// Its text form is 64 lower-case hex characters, which is what `GRAFTYARD_CUSTOMER_KEY` holds.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct KeyId(pub [u8; 32]);

impl KeyId {
    pub fn hex(&self) -> String {
        hex(&self.0)
    }

    pub fn parse_hex(s: &str) -> Option<KeyId> {
        Digest::parse_hex(s).map(|d| KeyId(d.0))
    }
}

impl fmt::Debug for KeyId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "KeyId({})", &self.hex()[..12])
    }
}

/// An Ed25519 signature.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Signature(pub [u8; 64]);

impl fmt::Debug for Signature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Signature({}..)", &hex(&self.0)[..12])
    }
}

pub(crate) fn hex(b: &[u8]) -> String {
    b.iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct IntentId(pub Ulid);

/// The whole key set. `ring` holds the account-separated sites' Ed25519 keys.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keys {
    pub generation: u64,
    pub customer: KeyId,
    /// `None` for self-hosted.
    pub vendor: Option<KeyId>,
    pub release: KeyId,
    pub drain: KeyId,
    pub executor: KeyId,
    pub agents_executor: Option<KeyId>,
    pub ring: SmallVec<[(SiteId, KeyId); 6]>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RestoreSource {
    Vault { account: String },
    Lifeboat { host: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestoreFrom {
    pub source: RestoreSource,
    pub cutoff: Timestamp,
}

/// The first entry an empty `Release` group accepts, and only when `install` and the customer signature match the
/// trust anchor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Genesis {
    pub install: InstallId,
    /// The install's own account, whose `Authz(account)` chain genesis creates.
    pub account: AccountId,
    pub keys: Keys,
    pub sites: SmallVec<[Site; 9]>,
    /// `Release`, `Names`, `Tokens`, and `Ids`, each once; changed only by `LeaveEvaluation`.
    pub placements: SmallVec<[(SystemChain, PlacementRecord); 4]>,
    /// The signed `evaluation` flag; only `LeaveEvaluation` clears it.
    pub evaluation: bool,
    pub restore: Option<RestoreFrom>,
}

impl Genesis {
    pub fn placement(&self, chain: SystemChain) -> Option<&PlacementRecord> {
        self.placements.iter().find(|(c, _)| *c == chain).map(|(_, r)| r)
    }
}

/// What every `Release` entry is signed over, with its body, so it verifies once, on one install, at one position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bound {
    pub install: InstallId,
    pub group: ChainId,
    /// Digest of the last committed signed entry; `None` only for genesis.
    pub prev: Option<Digest>,
    /// A pre-signed undo binds its parent intent instead of `prev`.
    pub parent: Option<IntentId>,
}

/// The Workers version id an upload returns: a UUID. Its text form is the UUID's, lower-case and hyphenated.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VersionId(pub [u8; 16]);

impl fmt::Display for VersionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let h = hex(&self.0);
        write!(f, "{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
    }
}

impl fmt::Debug for VersionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "VersionId({self})")
    }
}

impl FromStr for VersionId {
    type Err = ();
    fn from_str(s: &str) -> Result<VersionId, ()> {
        let h: String = s.chars().filter(|c| *c != '-').collect();
        if s.len() != 36 || h.len() != 32 {
            return Err(());
        }
        let mut out = [0u8; 16];
        for (i, o) in out.iter_mut().enumerate() {
            *o = u8::from_str_radix(h.get(2 * i..2 * i + 2).ok_or(())?, 16).map_err(|_| ())?;
        }
        Ok(VersionId(out))
    }
}

/// One stored record format, as `LogEntry.format` names it: a record family and its version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Format {
    pub family: u16,
    pub version: u16,
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.family, self.version)
    }
}

/// Sorted, no duplicates; the decoder refuses any other order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FormatSet(pub SmallVec<[Format; 32]>);

impl FormatSet {
    /// Sorts and drops duplicates.
    pub fn new(it: impl IntoIterator<Item = Format>) -> FormatSet {
        let mut v: SmallVec<[Format; 32]> = it.into_iter().collect();
        v.sort_unstable();
        v.dedup();
        FormatSet(v)
    }

    pub fn contains(&self, f: &Format) -> bool {
        self.0.binary_search(f).is_ok()
    }

    /// The bundle's formats file, `formats.txt`, whose SHA-256 the signed release record lists: one `family.version`
    /// line per format, in order, each ending in a newline.
    pub fn file(&self) -> String {
        self.0.iter().map(|f| format!("{f}\n")).collect()
    }
}

/// A digest-pinned image in the account's registry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageRef {
    pub name: Box<str>,
    pub digest: Digest,
}

/// The chains a `StaleMode` flag covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Install,
    Owner(AccountId),
    Repo(RepoId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Script {
    Site(SiteId),
    Workflows,
    Front,
    Edge,
    Ops,
    Mcp,
    AgentsFront,
    Controller,
}

/// Committed by `graft-ops` under the executor key after it verified the release signature over `record` and
/// uploaded every script as a version without deploying it. `record` is the release's `release.json`, so `release` is
/// its SHA-256, and the group checks `record_sig` under `Keys.release`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseUploaded {
    pub release: ReleaseId,
    pub record: Box<[u8]>,
    pub record_sig: Signature,
    pub versions: SmallVec<[(Script, VersionId); 16]>,
    pub images: SmallVec<[ImageRef; 4]>,
    pub reads: FormatSet,
}

impl ReleaseUploaded {
    pub fn version(&self, script: Script) -> Option<VersionId> {
        self.versions.iter().find(|(s, _)| *s == script).map(|(_, v)| *v)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Drain,
    Normal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SiteMode {
    pub site: SiteId,
    pub mode: Mode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DayTarget {
    Site(SiteId),
    Workflows,
    /// The undo of a `ContainerDay`: shards return to `git_prev`.
    Containers(SiteId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecretName {
    DeployToken,
    ReadToken,
    TunnelToken,
    MintWrapKey,
    SshHostKeys,
    McpAppKey,
    McpClientSecret,
    InsightsAppKey,
    AgentsAppKey,
    WorkspaceAppKey,
    SecretsWrapKey,
    HostedRunnersAppKey,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Intent {
    SiteDay { target: DayTarget, release: ReleaseId },
    ContainerDay { site: SiteId, release: ReleaseId },
    EdgeRoll { release: ReleaseId },
    FrontRoll { release: ReleaseId },
    OpsRoll { release: ReleaseId },
    McpRoll { release: ReleaseId },
    ControllerRoll { release: ReleaseId },
    ReplaceMember { lost: SiteId, spare: SiteId },
    RestorePlacement { site: SiteId },
    Rotate { secret: SecretName, slot: u8 },
    LeaveEvaluation(Box<LeaveEvaluation>),
}

/// `Intent::LeaveEvaluation`'s fields, boxed so every other intent stays small.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeaveEvaluation {
    pub sites: SmallVec<[Site; 6]>,
    pub placements: SmallVec<[(SystemChain, PlacementRecord); 4]>,
    /// The `Tri` records.
    pub default: PlacementRecord,
}

impl Intent {
    pub fn kind(&self) -> &'static str {
        match self {
            Intent::SiteDay { .. } => "SiteDay",
            Intent::ContainerDay { .. } => "ContainerDay",
            Intent::EdgeRoll { .. } => "EdgeRoll",
            Intent::FrontRoll { .. } => "FrontRoll",
            Intent::OpsRoll { .. } => "OpsRoll",
            Intent::McpRoll { .. } => "McpRoll",
            Intent::ControllerRoll { .. } => "ControllerRoll",
            Intent::ReplaceMember { .. } => "ReplaceMember",
            Intent::RestorePlacement { .. } => "RestorePlacement",
            Intent::Rotate { .. } => "Rotate",
            Intent::LeaveEvaluation(_) => "LeaveEvaluation",
        }
    }

    /// The release the intent rolls, if it names one.
    pub fn release(&self) -> Option<ReleaseId> {
        match self {
            Intent::SiteDay { release, .. }
            | Intent::ContainerDay { release, .. }
            | Intent::EdgeRoll { release }
            | Intent::FrontRoll { release }
            | Intent::OpsRoll { release }
            | Intent::McpRoll { release }
            | Intent::ControllerRoll { release } => Some(*release),
            _ => None,
        }
    }

    /// The pinned script the intent rolls, whose pointer its commit clears.
    pub fn pinned(&self) -> Option<Script> {
        match self {
            Intent::EdgeRoll { .. } => Some(Script::Edge),
            Intent::OpsRoll { .. } => Some(Script::Ops),
            Intent::McpRoll { .. } => Some(Script::Mcp),
            Intent::ControllerRoll { .. } => Some(Script::Controller),
            _ => None,
        }
    }
}

/// A human-signed intent. Its signer is the entry's one signature, over `Bound` and this body. Each `undo` is a
/// pre-signed undo entry with the signature of the same key over that entry bound to `parent: Some(id)` and no
/// `prev`, so it verifies only while this intent is current.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedIntent {
    pub id: IntentId,
    pub issued_at: Timestamp,
    pub not_after: Timestamp,
    pub intent: Intent,
    pub undo: Vec<(ReleaseEntry, Signature)>,
}

/// Where the current intent is. `step` counts the `IntentStep`s committed since it entered the state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntentState {
    Running { step: u8 },
    Halted { step: u8 },
    Undoing,
    Aborting { target: ReleaseId },
}

/// Every `Release` entry (`design/DESIGN.md`, Shape, "Release group"), with its tag in declaration order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReleaseEntry {
    Genesis(Box<Genesis>),
    Keys(Box<Keys>),
    Intent(Box<SignedIntent>),
    SiteMode(SiteMode),
    ReleaseUploaded(Box<ReleaseUploaded>),
    IntentStep {
        intent: IntentId,
        step: u8,
    },
    IntentDone {
        intent: IntentId,
    },
    SiteRelease {
        site: SiteId,
        release: ReleaseId,
    },
    /// Undo only.
    Rollback {
        target: DayTarget,
        release: ReleaseId,
    },
    /// Undo only.
    EdgePointer {
        pin: ReleaseId,
    },
    /// Undo only.
    OpsPointer {
        pin: ReleaseId,
    },
    /// Undo only.
    McpPointer {
        pin: ReleaseId,
    },
    /// Undo only.
    ControllerPointer {
        pin: ReleaseId,
    },
    SiteLost {
        site: SiteId,
    },
    StaleMode {
        scope: Scope,
        until: Timestamp,
    },
    LifeboatAdd {
        host: Box<str>,
        key: KeyId,
    },
    LifeboatRemove {
        host: Box<str>,
    },
    LifeboatDecoder {
        host: Box<str>,
        release: ReleaseId,
    },
    FormatsEnable {
        format: Format,
    },
    RepoRestore {
        repos: Vec<RepoId>,
        refs: Vec<Box<str>>,
        at: Timestamp,
        reason: Box<str>,
    },
    RepoReset {
        repo: RepoId,
        at: Timestamp,
        reason: Box<str>,
    },
    Halt {
        intent: IntentId,
    },
    Resume {
        intent: IntentId,
        undo: Vec<(ReleaseEntry, Signature)>,
    },
    Abort {
        intent: IntentId,
        target: ReleaseId,
    },
}

impl ReleaseEntry {
    pub fn kind(&self) -> &'static str {
        match self {
            ReleaseEntry::Genesis(_) => "Genesis",
            ReleaseEntry::Keys(_) => "Keys",
            ReleaseEntry::Intent(_) => "Intent",
            ReleaseEntry::SiteMode(_) => "SiteMode",
            ReleaseEntry::ReleaseUploaded(_) => "ReleaseUploaded",
            ReleaseEntry::IntentStep { .. } => "IntentStep",
            ReleaseEntry::IntentDone { .. } => "IntentDone",
            ReleaseEntry::SiteRelease { .. } => "SiteRelease",
            ReleaseEntry::Rollback { .. } => "Rollback",
            ReleaseEntry::EdgePointer { .. } => "EdgePointer",
            ReleaseEntry::OpsPointer { .. } => "OpsPointer",
            ReleaseEntry::McpPointer { .. } => "McpPointer",
            ReleaseEntry::ControllerPointer { .. } => "ControllerPointer",
            ReleaseEntry::SiteLost { .. } => "SiteLost",
            ReleaseEntry::StaleMode { .. } => "StaleMode",
            ReleaseEntry::LifeboatAdd { .. } => "LifeboatAdd",
            ReleaseEntry::LifeboatRemove { .. } => "LifeboatRemove",
            ReleaseEntry::LifeboatDecoder { .. } => "LifeboatDecoder",
            ReleaseEntry::FormatsEnable { .. } => "FormatsEnable",
            ReleaseEntry::RepoRestore { .. } => "RepoRestore",
            ReleaseEntry::RepoReset { .. } => "RepoReset",
            ReleaseEntry::Halt { .. } => "Halt",
            ReleaseEntry::Resume { .. } => "Resume",
            ReleaseEntry::Abort { .. } => "Abort",
        }
    }

    /// The variants that commit only as a pre-signed undo of the current intent.
    pub fn undo_only(&self) -> bool {
        matches!(
            self,
            ReleaseEntry::Rollback { .. }
                | ReleaseEntry::EdgePointer { .. }
                | ReleaseEntry::OpsPointer { .. }
                | ReleaseEntry::McpPointer { .. }
                | ReleaseEntry::ControllerPointer { .. }
        )
    }

    /// A pointer undo's script and pinned release.
    pub fn pointer(&self) -> Option<(Script, ReleaseId)> {
        match self {
            ReleaseEntry::EdgePointer { pin } => Some((Script::Edge, *pin)),
            ReleaseEntry::OpsPointer { pin } => Some((Script::Ops, *pin)),
            ReleaseEntry::McpPointer { pin } => Some((Script::Mcp, *pin)),
            ReleaseEntry::ControllerPointer { pin } => Some((Script::Controller, *pin)),
            _ => None,
        }
    }
}

// ---- Encoding ----

fn ulid(e: &mut Enc, u: Ulid) {
    e.u128(u.0);
}

fn key(e: &mut Enc, k: &KeyId) {
    e.raw(&k.0);
}

fn region_tag(r: Region) -> u8 {
    match r {
        Region::Enam => 0,
        Region::Wnam => 1,
        Region::Weur => 2,
        Region::Eeur => 3,
        Region::Apac => 4,
        Region::Oc => 5,
    }
}

fn site(e: &mut Enc, s: &Site) {
    e.u8(s.id.0);
    e.str(&s.script);
    e.str(&s.account);
    e.u8(region_tag(s.region));
    e.option(s.jurisdiction.as_ref(), |e, j| {
        e.u8(match j {
            Jurisdiction::Eu => 0,
            Jurisdiction::Us => 1,
        })
    });
    e.str(&s.bucket);
    e.option(s.release.as_ref(), |e, r| e.raw(&r.0));
    match &s.link {
        SiteLink::Binding { member_class } => {
            e.u8(0);
            e.str(member_class);
        }
        SiteLink::Https { ring_host, key } => {
            e.u8(1);
            e.str(ring_host);
            e.raw(key);
        }
    }
    e.bool(s.spare);
}

fn name_tag(n: PlacementName) -> u8 {
    match n {
        PlacementName::Tri => 0,
        PlacementName::Na3 => 1,
        PlacementName::Us2w => 2,
        PlacementName::Eu2w => 3,
        PlacementName::Apac2w => 4,
        PlacementName::Solo => 5,
    }
}

/// The same bytes `crates/identity` writes for a record inside a `Names` command.
pub(crate) fn record(e: &mut Enc, r: &PlacementRecord) {
    e.u8(name_tag(r.name));
    e.count(r.sites.len());
    for s in &r.sites {
        e.u8(s.0);
    }
    e.u8(r.home.0);
    e.u32(r.generation);
    e.option(r.moving_from.as_deref(), record);
    e.u64(r.at.0);
}

fn system_chain_tag(c: SystemChain) -> u8 {
    match c {
        SystemChain::Release => 0,
        SystemChain::Names => 1,
        SystemChain::Tokens => 2,
        SystemChain::Ids => 3,
    }
}

pub(crate) fn keys(e: &mut Enc, k: &Keys) {
    e.u64(k.generation);
    key(e, &k.customer);
    e.option(k.vendor.as_ref(), key);
    key(e, &k.release);
    key(e, &k.drain);
    key(e, &k.executor);
    e.option(k.agents_executor.as_ref(), key);
    e.count(k.ring.len());
    for (s, k) in &k.ring {
        e.u8(s.0);
        key(e, k);
    }
}

pub(crate) fn genesis(e: &mut Enc, g: &Genesis) {
    ulid(e, g.install.0);
    e.u64(g.account.0.0 as u64);
    keys(e, &g.keys);
    e.count(g.sites.len());
    for s in &g.sites {
        site(e, s);
    }
    e.count(g.placements.len());
    for (c, r) in &g.placements {
        e.u8(system_chain_tag(*c));
        record(e, r);
    }
    e.bool(g.evaluation);
    e.option(g.restore.as_ref(), |e, r| {
        match &r.source {
            RestoreSource::Vault { account } => {
                e.u8(0);
                e.str(account);
            }
            RestoreSource::Lifeboat { host } => {
                e.u8(1);
                e.str(host);
            }
        }
        e.u64(r.cutoff.0);
    });
}

pub(crate) fn bound(e: &mut Enc, b: &Bound) {
    ulid(e, b.install.0);
    e.str(&b.group.to_string());
    e.option(b.prev.as_ref(), |e, d| e.raw(&d.0));
    e.option(b.parent.as_ref(), |e, p| ulid(e, p.0));
}

fn release_id(e: &mut Enc, r: &ReleaseId) {
    e.raw(&r.0);
}

fn site_id(e: &mut Enc, s: SiteId) {
    e.u8(s.0);
}

fn db_id(e: &mut Enc, id: DbId) {
    e.u64(id.0 as u64);
}

fn format(e: &mut Enc, f: &Format) {
    e.u32((u32::from(f.family) << 16) | u32::from(f.version));
}

fn day_target(e: &mut Enc, t: &DayTarget) {
    match t {
        DayTarget::Site(s) => {
            e.u8(0);
            site_id(e, *s);
        }
        DayTarget::Workflows => e.u8(1),
        DayTarget::Containers(s) => {
            e.u8(2);
            site_id(e, *s);
        }
    }
}

fn script(e: &mut Enc, s: &Script) {
    match s {
        Script::Site(id) => {
            e.u8(0);
            site_id(e, *id);
        }
        Script::Workflows => e.u8(1),
        Script::Front => e.u8(2),
        Script::Edge => e.u8(3),
        Script::Ops => e.u8(4),
        Script::Mcp => e.u8(5),
        Script::AgentsFront => e.u8(6),
        Script::Controller => e.u8(7),
    }
}

const SECRETS: [SecretName; 12] = [
    SecretName::DeployToken,
    SecretName::ReadToken,
    SecretName::TunnelToken,
    SecretName::MintWrapKey,
    SecretName::SshHostKeys,
    SecretName::McpAppKey,
    SecretName::McpClientSecret,
    SecretName::InsightsAppKey,
    SecretName::AgentsAppKey,
    SecretName::WorkspaceAppKey,
    SecretName::SecretsWrapKey,
    SecretName::HostedRunnersAppKey,
];

fn intent(e: &mut Enc, i: &Intent) {
    match i {
        Intent::SiteDay { target, release } => {
            e.u8(0);
            day_target(e, target);
            release_id(e, release);
        }
        Intent::ContainerDay { site, release } => {
            e.u8(1);
            site_id(e, *site);
            release_id(e, release);
        }
        Intent::EdgeRoll { release } => {
            e.u8(2);
            release_id(e, release);
        }
        Intent::FrontRoll { release } => {
            e.u8(3);
            release_id(e, release);
        }
        Intent::OpsRoll { release } => {
            e.u8(4);
            release_id(e, release);
        }
        Intent::McpRoll { release } => {
            e.u8(5);
            release_id(e, release);
        }
        Intent::ControllerRoll { release } => {
            e.u8(6);
            release_id(e, release);
        }
        Intent::ReplaceMember { lost, spare } => {
            e.u8(7);
            site_id(e, *lost);
            site_id(e, *spare);
        }
        Intent::RestorePlacement { site } => {
            e.u8(8);
            site_id(e, *site);
        }
        Intent::Rotate { secret, slot } => {
            e.u8(9);
            e.u8(SECRETS.iter().position(|s| s == secret).expect("every secret is listed") as u8);
            e.u8(*slot);
        }
        Intent::LeaveEvaluation(l) => {
            let LeaveEvaluation { sites, placements, default } = &**l;
            e.u8(10);
            e.count(sites.len());
            for s in sites {
                site(e, s);
            }
            e.count(placements.len());
            for (c, r) in placements {
                e.u8(system_chain_tag(*c));
                record(e, r);
            }
            record(e, default);
        }
    }
}

fn undo_list(e: &mut Enc, undo: &[(ReleaseEntry, Signature)]) {
    e.count(undo.len());
    for (u, s) in undo {
        entry(e, u);
        e.raw(&s.0);
    }
}

fn host(e: &mut Enc, h: &str) {
    e.str(h);
}

pub(crate) fn entry(e: &mut Enc, r: &ReleaseEntry) {
    match r {
        ReleaseEntry::Genesis(g) => {
            e.u8(0);
            genesis(e, g);
        }
        ReleaseEntry::Keys(k) => {
            e.u8(1);
            keys(e, k);
        }
        ReleaseEntry::Intent(i) => {
            e.u8(2);
            ulid(e, i.id.0);
            e.u64(i.issued_at.0);
            e.u64(i.not_after.0);
            intent(e, &i.intent);
            undo_list(e, &i.undo);
        }
        ReleaseEntry::SiteMode(m) => {
            e.u8(3);
            site_id(e, m.site);
            e.u8(match m.mode {
                Mode::Drain => 0,
                Mode::Normal => 1,
            });
        }
        ReleaseEntry::ReleaseUploaded(u) => {
            e.u8(4);
            release_id(e, &u.release);
            e.count(u.record.len());
            e.raw(&u.record);
            e.raw(&u.record_sig.0);
            e.count(u.versions.len());
            for (s, v) in &u.versions {
                script(e, s);
                e.raw(&v.0);
            }
            e.count(u.images.len());
            for i in &u.images {
                e.str(&i.name);
                e.raw(&i.digest.0);
            }
            e.count(u.reads.0.len());
            for f in &u.reads.0 {
                format(e, f);
            }
        }
        ReleaseEntry::IntentStep { intent, step } => {
            e.u8(5);
            ulid(e, intent.0);
            e.u8(*step);
        }
        ReleaseEntry::IntentDone { intent } => {
            e.u8(6);
            ulid(e, intent.0);
        }
        ReleaseEntry::SiteRelease { site, release } => {
            e.u8(7);
            site_id(e, *site);
            release_id(e, release);
        }
        ReleaseEntry::Rollback { target, release } => {
            e.u8(8);
            day_target(e, target);
            release_id(e, release);
        }
        ReleaseEntry::EdgePointer { pin } => {
            e.u8(9);
            release_id(e, pin);
        }
        ReleaseEntry::OpsPointer { pin } => {
            e.u8(10);
            release_id(e, pin);
        }
        ReleaseEntry::McpPointer { pin } => {
            e.u8(11);
            release_id(e, pin);
        }
        ReleaseEntry::ControllerPointer { pin } => {
            e.u8(12);
            release_id(e, pin);
        }
        ReleaseEntry::SiteLost { site } => {
            e.u8(13);
            site_id(e, *site);
        }
        ReleaseEntry::StaleMode { scope, until } => {
            e.u8(14);
            match scope {
                Scope::Install => e.u8(0),
                Scope::Owner(a) => {
                    e.u8(1);
                    db_id(e, a.0);
                }
                Scope::Repo(r) => {
                    e.u8(2);
                    db_id(e, r.0);
                }
            }
            e.u64(until.0);
        }
        ReleaseEntry::LifeboatAdd { host: h, key: k } => {
            e.u8(15);
            host(e, h);
            key(e, k);
        }
        ReleaseEntry::LifeboatRemove { host: h } => {
            e.u8(16);
            host(e, h);
        }
        ReleaseEntry::LifeboatDecoder { host: h, release } => {
            e.u8(17);
            host(e, h);
            release_id(e, release);
        }
        ReleaseEntry::FormatsEnable { format: f } => {
            e.u8(18);
            format(e, f);
        }
        ReleaseEntry::RepoRestore { repos, refs, at, reason } => {
            e.u8(19);
            e.count(repos.len());
            for r in repos {
                db_id(e, r.0);
            }
            e.count(refs.len());
            for r in refs {
                e.str(r);
            }
            e.u64(at.0);
            e.str(reason);
        }
        ReleaseEntry::RepoReset { repo, at, reason } => {
            e.u8(20);
            db_id(e, repo.0);
            e.u64(at.0);
            e.str(reason);
        }
        ReleaseEntry::Halt { intent } => {
            e.u8(21);
            ulid(e, intent.0);
        }
        ReleaseEntry::Resume { intent, undo } => {
            e.u8(22);
            ulid(e, intent.0);
            undo_list(e, undo);
        }
        ReleaseEntry::Abort { intent, target } => {
            e.u8(23);
            ulid(e, intent.0);
            release_id(e, target);
        }
    }
}

// ---- Decoding ----

impl Dec<'_> {
    fn ulid(&mut self) -> Result<Ulid, Malformed> {
        let v = self.u128()?;
        // The text form has 26 characters of 5 bits, so the top two of 130 bits are zero; every u128 is valid.
        Ok(Ulid(v))
    }

    fn key(&mut self) -> Result<KeyId, Malformed> {
        self.array().map(KeyId)
    }

    fn string(&mut self) -> Result<String, Malformed> {
        self.str(STR_MAX).map(str::to_owned)
    }

    fn region(&mut self) -> Result<Region, Malformed> {
        Ok(match self.u8()? {
            0 => Region::Enam,
            1 => Region::Wnam,
            2 => Region::Weur,
            3 => Region::Eeur,
            4 => Region::Apac,
            5 => Region::Oc,
            t => return bad(format!("region tag {t}")),
        })
    }

    fn site(&mut self) -> Result<Site, Malformed> {
        let id = SiteId(self.u8()?);
        let script = self.string()?;
        let account = self.string()?;
        let region = self.region()?;
        let jurisdiction = self.option(|d| match d.u8()? {
            0 => Ok(Jurisdiction::Eu),
            1 => Ok(Jurisdiction::Us),
            t => bad(format!("jurisdiction tag {t}")),
        })?;
        let bucket = self.string()?;
        let release = self.option(|d| d.array().map(ReleaseId))?;
        let link = match self.u8()? {
            0 => SiteLink::Binding { member_class: self.string()? },
            1 => SiteLink::Https { ring_host: self.string()?, key: self.array()? },
            t => return bad(format!("site link tag {t}")),
        };
        let spare = self.bool()?;
        Ok(Site { id, script, account, region, jurisdiction, bucket, release, link, spare })
    }

    fn record(&mut self, depth: u8) -> Result<PlacementRecord, Malformed> {
        let name = match self.u8()? {
            0 => PlacementName::Tri,
            1 => PlacementName::Na3,
            2 => PlacementName::Us2w,
            3 => PlacementName::Eu2w,
            4 => PlacementName::Apac2w,
            5 => PlacementName::Solo,
            t => return bad(format!("placement name tag {t}")),
        };
        let n = self.count(3)?;
        let sites = (0..n).map(|_| self.u8().map(SiteId)).collect::<Result<_, _>>()?;
        let home = SiteId(self.u8()?);
        let generation = self.u32()?;
        // Records nest one level per move in progress; a forged chain of them cannot recurse without bound.
        let moving_from = if depth >= 8 {
            match self.u8()? {
                0 => None,
                _ => return bad("moving_from nested past 8 records"),
            }
        } else {
            self.option(|d| d.record(depth + 1).map(Box::new))?
        };
        let at = Timestamp(self.u64()?);
        Ok(PlacementRecord { name, sites, home, generation, moving_from, at })
    }

    fn keys(&mut self) -> Result<Keys, Malformed> {
        let generation = self.u64()?;
        let customer = self.key()?;
        let vendor = self.option(Dec::key)?;
        let release = self.key()?;
        let drain = self.key()?;
        let executor = self.key()?;
        let agents_executor = self.option(Dec::key)?;
        let n = self.count(64)?;
        let ring = (0..n).map(|_| Ok((SiteId(self.u8()?), self.key()?))).collect::<Result<_, Malformed>>()?;
        Ok(Keys { generation, customer, vendor, release, drain, executor, agents_executor, ring })
    }

    fn genesis(&mut self) -> Result<Genesis, Malformed> {
        let install = InstallId(self.ulid()?);
        let account = self.u64()?;
        if account >= 1 << 53 {
            return bad(format!("account id {account} is not below 2^53"));
        }
        let account = AccountId(DbId(account as i64));
        let keys = self.keys()?;
        let n = self.count(64)?;
        let sites = (0..n).map(|_| self.site()).collect::<Result<_, _>>()?;
        let n = self.count(4)?;
        let placements = (0..n)
            .map(|_| {
                let c = match self.u8()? {
                    0 => SystemChain::Release,
                    1 => SystemChain::Names,
                    2 => SystemChain::Tokens,
                    3 => SystemChain::Ids,
                    t => return bad(format!("system chain tag {t}")),
                };
                Ok((c, self.record(0)?))
            })
            .collect::<Result<_, Malformed>>()?;
        let evaluation = self.bool()?;
        let restore = self.option(|d| {
            let source = match d.u8()? {
                0 => RestoreSource::Vault { account: d.string()? },
                1 => RestoreSource::Lifeboat { host: d.string()? },
                t => return bad(format!("restore source tag {t}")),
            };
            Ok(RestoreFrom { source, cutoff: Timestamp(d.u64()?) })
        })?;
        Ok(Genesis { install, account, keys, sites, placements, evaluation, restore })
    }

    pub(crate) fn bound(&mut self) -> Result<Bound, Malformed> {
        let install = InstallId(self.ulid()?);
        let group = self.str(64)?;
        let group: ChainId = group.parse().or_else(|_| bad(format!("bound group {group:?}")))?;
        let prev = self.option(|d| d.array().map(Digest))?;
        let parent = self.option(|d| d.ulid().map(IntentId))?;
        Ok(Bound { install, group, prev, parent })
    }

    fn release_id(&mut self) -> Result<ReleaseId, Malformed> {
        self.array().map(ReleaseId)
    }

    fn site_id(&mut self) -> Result<SiteId, Malformed> {
        let s = self.u8()?;
        if s >= 64 {
            return bad(format!("site id {s} is not below 64"));
        }
        Ok(SiteId(s))
    }

    fn db_id(&mut self) -> Result<DbId, Malformed> {
        let v = self.u64()?;
        if v >= 1 << 53 {
            return bad(format!("id {v} is not below 2^53"));
        }
        Ok(DbId(v as i64))
    }

    fn format(&mut self) -> Result<Format, Malformed> {
        let v = self.u32()?;
        Ok(Format { family: (v >> 16) as u16, version: v as u16 })
    }

    fn day_target(&mut self) -> Result<DayTarget, Malformed> {
        Ok(match self.u8()? {
            0 => DayTarget::Site(self.site_id()?),
            1 => DayTarget::Workflows,
            2 => DayTarget::Containers(self.site_id()?),
            t => return bad(format!("day target tag {t}")),
        })
    }

    fn script(&mut self) -> Result<Script, Malformed> {
        Ok(match self.u8()? {
            0 => Script::Site(self.site_id()?),
            1 => Script::Workflows,
            2 => Script::Front,
            3 => Script::Edge,
            4 => Script::Ops,
            5 => Script::Mcp,
            6 => Script::AgentsFront,
            7 => Script::Controller,
            t => return bad(format!("script tag {t}")),
        })
    }

    fn host(&mut self) -> Result<Box<str>, Malformed> {
        let h = self.str(253)?;
        if h.is_empty() {
            return bad("an empty host name");
        }
        Ok(h.into())
    }

    fn intent(&mut self) -> Result<Intent, Malformed> {
        Ok(match self.u8()? {
            0 => Intent::SiteDay { target: self.day_target()?, release: self.release_id()? },
            1 => Intent::ContainerDay { site: self.site_id()?, release: self.release_id()? },
            2 => Intent::EdgeRoll { release: self.release_id()? },
            3 => Intent::FrontRoll { release: self.release_id()? },
            4 => Intent::OpsRoll { release: self.release_id()? },
            5 => Intent::McpRoll { release: self.release_id()? },
            6 => Intent::ControllerRoll { release: self.release_id()? },
            7 => Intent::ReplaceMember { lost: self.site_id()?, spare: self.site_id()? },
            8 => Intent::RestorePlacement { site: self.site_id()? },
            9 => {
                let t = self.u8()?;
                let secret = *SECRETS.get(t as usize).ok_or_else(|| Malformed(format!("secret tag {t}").into()))?;
                Intent::Rotate { secret, slot: self.u8()? }
            }
            10 => {
                let n = self.count(64)?;
                let sites = (0..n).map(|_| self.site()).collect::<Result<_, _>>()?;
                let n = self.count(4)?;
                let placements =
                    (0..n).map(|_| Ok((self.system_chain()?, self.record(0)?))).collect::<Result<_, Malformed>>()?;
                Intent::LeaveEvaluation(Box::new(LeaveEvaluation { sites, placements, default: self.record(0)? }))
            }
            t => return bad(format!("intent tag {t}")),
        })
    }

    /// A pre-signed undo list. An undo is never an intent or a `Resume`, so the lists do not nest.
    fn undo_list(&mut self) -> Result<Vec<(ReleaseEntry, Signature)>, Malformed> {
        let n = self.count(8)?;
        (0..n)
            .map(|_| {
                let at = self.at;
                let u = self.entry()?;
                if !u.undo_only() {
                    return bad(format!("a {} at offset {at} is not an undo", u.kind()));
                }
                Ok((u, Signature(self.array()?)))
            })
            .collect()
    }

    fn system_chain(&mut self) -> Result<SystemChain, Malformed> {
        Ok(match self.u8()? {
            0 => SystemChain::Release,
            1 => SystemChain::Names,
            2 => SystemChain::Tokens,
            3 => SystemChain::Ids,
            t => return bad(format!("system chain tag {t}")),
        })
    }

    pub(crate) fn entry(&mut self) -> Result<ReleaseEntry, Malformed> {
        Ok(match self.u8()? {
            0 => ReleaseEntry::Genesis(Box::new(self.genesis()?)),
            1 => ReleaseEntry::Keys(Box::new(self.keys()?)),
            2 => {
                let id = IntentId(self.ulid()?);
                let issued_at = Timestamp(self.u64()?);
                let not_after = Timestamp(self.u64()?);
                let intent = self.intent()?;
                let undo = self.undo_list()?;
                ReleaseEntry::Intent(Box::new(SignedIntent { id, issued_at, not_after, intent, undo }))
            }
            3 => {
                let site = self.site_id()?;
                let mode = match self.u8()? {
                    0 => Mode::Drain,
                    1 => Mode::Normal,
                    t => return bad(format!("mode tag {t}")),
                };
                ReleaseEntry::SiteMode(SiteMode { site, mode })
            }
            4 => {
                let release = self.release_id()?;
                let n = self.count(RECORD_MAX)?;
                let record = self.take(n)?.into();
                let record_sig = Signature(self.array()?);
                let n = self.count(16)?;
                let versions =
                    (0..n).map(|_| Ok((self.script()?, VersionId(self.array()?)))).collect::<Result<_, Malformed>>()?;
                let n = self.count(4)?;
                let images = (0..n)
                    .map(|_| Ok(ImageRef { name: self.str(STR_MAX)?.into(), digest: Digest(self.array()?) }))
                    .collect::<Result<_, Malformed>>()?;
                let n = self.count(1024)?;
                let reads: SmallVec<[Format; 32]> = (0..n).map(|_| self.format()).collect::<Result<_, _>>()?;
                if !reads.windows(2).all(|w| w[0] < w[1]) {
                    return bad("reads is not sorted without duplicates");
                }
                ReleaseEntry::ReleaseUploaded(Box::new(ReleaseUploaded {
                    release,
                    record,
                    record_sig,
                    versions,
                    images,
                    reads: FormatSet(reads),
                }))
            }
            5 => ReleaseEntry::IntentStep { intent: IntentId(self.ulid()?), step: self.u8()? },
            6 => ReleaseEntry::IntentDone { intent: IntentId(self.ulid()?) },
            7 => ReleaseEntry::SiteRelease { site: self.site_id()?, release: self.release_id()? },
            8 => ReleaseEntry::Rollback { target: self.day_target()?, release: self.release_id()? },
            9 => ReleaseEntry::EdgePointer { pin: self.release_id()? },
            10 => ReleaseEntry::OpsPointer { pin: self.release_id()? },
            11 => ReleaseEntry::McpPointer { pin: self.release_id()? },
            12 => ReleaseEntry::ControllerPointer { pin: self.release_id()? },
            13 => ReleaseEntry::SiteLost { site: self.site_id()? },
            14 => {
                let scope = match self.u8()? {
                    0 => Scope::Install,
                    1 => Scope::Owner(AccountId(self.db_id()?)),
                    2 => Scope::Repo(RepoId(self.db_id()?)),
                    t => return bad(format!("scope tag {t}")),
                };
                ReleaseEntry::StaleMode { scope, until: Timestamp(self.u64()?) }
            }
            15 => ReleaseEntry::LifeboatAdd { host: self.host()?, key: self.key()? },
            16 => ReleaseEntry::LifeboatRemove { host: self.host()? },
            17 => ReleaseEntry::LifeboatDecoder { host: self.host()?, release: self.release_id()? },
            18 => ReleaseEntry::FormatsEnable { format: self.format()? },
            19 => {
                let n = self.count(1024)?;
                let repos = (0..n).map(|_| self.db_id().map(RepoId)).collect::<Result<_, _>>()?;
                let n = self.count(1024)?;
                let refs = (0..n).map(|_| self.str(STR_MAX).map(Into::into)).collect::<Result<_, _>>()?;
                let at = Timestamp(self.u64()?);
                ReleaseEntry::RepoRestore { repos, refs, at, reason: self.str(STR_MAX)?.into() }
            }
            20 => ReleaseEntry::RepoReset {
                repo: RepoId(self.db_id()?),
                at: Timestamp(self.u64()?),
                reason: self.str(STR_MAX)?.into(),
            },
            21 => ReleaseEntry::Halt { intent: IntentId(self.ulid()?) },
            22 => ReleaseEntry::Resume { intent: IntentId(self.ulid()?), undo: self.undo_list()? },
            23 => ReleaseEntry::Abort { intent: IntentId(self.ulid()?), target: self.release_id()? },
            t => return bad(format!("release entry tag {t} is unknown")),
        })
    }
}
