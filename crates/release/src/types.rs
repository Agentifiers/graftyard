//! The `Release` group types genesis acceptance needs (`design/DESIGN.md`, Shape, "Release group"), and their
//! canonical bytes. Enum tags follow the design's declaration order, so the variants built later keep their places.

use std::fmt;
use std::str::FromStr;

use graftyard_types::{
    AccountId, ChainId, DbId, Digest, Jurisdiction, PlacementName, PlacementRecord, Region, ReleaseId, Site, SiteId,
    SiteLink, SystemChain, Timestamp, Ulid,
};
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

use crate::codec::{Dec, Enc, Malformed, bad};

/// Longest string field a decoder takes: script, account, bucket, class, and host names are far shorter.
const STR_MAX: usize = 1024;

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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

/// The design's `ReleaseEntry`, with the variants genesis acceptance needs. A later variant takes the tag of its
/// position in the design's declaration, and a decoder that meets one answers `Malformed`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReleaseEntry {
    Genesis(Box<Genesis>),
    Keys(Box<Keys>),
}

impl ReleaseEntry {
    pub fn kind(&self) -> &'static str {
        match self {
            ReleaseEntry::Genesis(_) => "Genesis",
            ReleaseEntry::Keys(_) => "Keys",
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

    pub(crate) fn entry(&mut self) -> Result<ReleaseEntry, Malformed> {
        Ok(match self.u8()? {
            0 => ReleaseEntry::Genesis(Box::new(self.genesis()?)),
            1 => ReleaseEntry::Keys(Box::new(self.keys()?)),
            t => return bad(format!("release entry tag {t} is not built yet or unknown")),
        })
    }
}
