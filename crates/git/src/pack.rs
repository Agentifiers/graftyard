//! Packs (`design/DESIGN.md`, Shape, "Packs"): ids, `PackRef`, `DurablePack`, `Promoted`, `Archived`, and the
//! `Committed` marker body.

use std::fmt;
use std::str::FromStr;
use std::time::Duration;

use graftyard_types::{ChainId, Digest, Index, ParseUlidError, RequestId, SiteSet, Timestamp, Ulid};
use serde::{Deserialize, Serialize};

use crate::key::R2Key;

/// Packs up to this size are inline in their entry.
pub const INLINE_MAX: usize = 1 << 20;
/// On a hot group, agent pushes inline only up to this (`design/DESIGN.md`, Shape, `INLINE_MAX_HOT`).
pub const INLINE_MAX_HOT: usize = 64 << 10;
/// `decide` refuses a stored pack whose `PackId` time is older than this, so a pack that has not committed by then
/// never will [guess].
pub const PACK_COMMIT_WINDOW: Duration = Duration::from_secs(3600);
/// The sweeper deletes an unmarked object only once it is this old.
pub const ORPHAN_AFTER: Duration = Duration::from_secs(24 * 3600);
/// Kept by each group's `request` table after the entry's `at` (`design/DESIGN.md`, Shape, the write path) [guess].
pub const REQUEST_RETENTION: Duration = Duration::from_secs(48 * 3600);

/// Fresh for every placement. Its ULID time is the placement's start, which `decide` and the sweeper read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PackId(pub Ulid);

impl PackId {
    pub fn time(self) -> Timestamp {
        self.0.time()
    }
}

impl fmt::Display for PackId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for PackId {
    type Err = ParseUlidError;
    fn from_str(s: &str) -> Result<PackId, ParseUlidError> {
        s.parse().map(PackId)
    }
}

/// `Inline` lives in the entry. `Stored` names the buckets of the sites that held a verified copy when the pack
/// became durable; the third-bucket filler may add the third later without changing the entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackRef {
    Inline(#[serde(with = "b64")] Vec<u8>),
    Stored { id: PackId, checksum: Digest, bytes: u64, sites: SiteSet },
}

impl PackRef {
    pub fn stored_id(&self) -> Option<PackId> {
        match self {
            PackRef::Stored { id, .. } => Some(*id),
            PackRef::Inline(_) => None,
        }
    }
}

/// Built only by `PackStore::place`: an inline pack within its cap, or a stored pack whose bytes are in the buckets
/// of two sites in two regions (one for `Solo`), each put create-only with its SHA-256, and whose `index-pack
/// --strict` passed on the same bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurablePack {
    pack: PackRef,
    _sealed: (),
}

impl DurablePack {
    pub(crate) fn seal(pack: PackRef) -> DurablePack {
        DurablePack { pack, _sealed: () }
    }

    pub fn pack(&self) -> &PackRef {
        &self.pack
    }

    pub fn into_pack(self) -> PackRef {
        self.pack
    }
}

/// What `GitCacheShard::validate` hands back: `index-pack --strict` and the connectivity check passed on bytes whose
/// SHA-256 is `checksum`. `PackStore::place` takes it as the proof that the bytes it places were validated.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackIndex {
    pub checksum: Digest,
    pub bytes: u64,
    pub objects: u32,
    /// The pack's own trailer (SHA-1 of its content), as `index-pack` prints it.
    pub trailer: Box<str>,
}

/// Packs the objects of every inline pack in `from..=to` into one stored pack. The leader commits `Archived` for a
/// range only after `Promoted` covers every inline pack the range names.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Promoted {
    pub from: Index,
    pub to: Index,
    /// Always `Stored`.
    pub pack: PackRef,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Archived {
    pub key: R2Key,
    pub from: Index,
    pub to: Index,
    pub from_at: Timestamp,
    pub to_at: Timestamp,
    pub digest: Digest,
}

/// The body of a `Committed` marker: the entry that names the pack, and the pack's checksum, so the third-bucket
/// filler can verify a copy without reading the chain. The checksum is an addition to the design's "(chain, index)".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Marker {
    pub chain: ChainId,
    pub index: Index,
    pub checksum: Digest,
    pub bytes: u64,
}

/// Each stored object's R2 custom metadata names its chain and `RequestId`, which is how the sweeper finds the
/// `request` row that proves a commit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObjectMeta {
    pub chain: ChainId,
    pub request: RequestId,
}

impl ObjectMeta {
    pub const CHAIN: &'static str = "chain";
    pub const REQUEST: &'static str = "request";

    pub fn to_pairs(self) -> [(String, String); 2] {
        [(Self::CHAIN.into(), self.chain.to_string()), (Self::REQUEST.into(), self.request.to_string())]
    }

    /// `None` when either field is missing or does not parse: the sweeper keeps and alarms such an object.
    pub fn from_pairs<'a>(pairs: impl IntoIterator<Item = (&'a str, &'a str)>) -> Option<ObjectMeta> {
        let (mut chain, mut request) = (None, None);
        for (k, v) in pairs {
            match k {
                Self::CHAIN => chain = v.parse().ok(),
                Self::REQUEST => request = v.parse().ok(),
                _ => {}
            }
        }
        Some(ObjectMeta { chain: chain?, request: request? })
    }
}

pub mod b64 {
    use serde::{Deserialize, Deserializer, Serializer};

    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    pub fn encode(b: &[u8]) -> String {
        let mut s = String::with_capacity(b.len().div_ceil(3) * 4);
        for c in b.chunks(3) {
            let n = (u32::from(c[0]) << 16)
                | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
                | u32::from(*c.get(2).unwrap_or(&0));
            for i in 0..4 {
                if i <= c.len() {
                    s.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
                } else {
                    s.push('=');
                }
            }
        }
        s
    }

    pub fn decode(s: &str) -> Option<Vec<u8>> {
        let s = s.as_bytes();
        if !s.len().is_multiple_of(4) {
            return None;
        }
        let mut out = Vec::with_capacity(s.len() / 4 * 3);
        for c in s.chunks(4) {
            let pad = c.iter().rev().take_while(|b| **b == b'=').count();
            if pad > 2 {
                return None;
            }
            let mut n = 0u32;
            for (i, b) in c.iter().enumerate() {
                let v = if i >= 4 - pad { 0 } else { A.iter().position(|a| a == b)? as u32 };
                n = (n << 6) | v;
            }
            let bytes = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
            out.extend_from_slice(&bytes[..3 - pad]);
        }
        Some(out)
    }

    pub fn serialize<S: Serializer>(b: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&encode(b))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        decode(&s).ok_or_else(|| serde::de::Error::custom("bad base64"))
    }
}

#[cfg(test)]
mod tests {
    use graftyard_types::{DbId, RepoId};

    use super::*;

    #[test]
    fn base64_round_trips_every_tail_length() {
        for n in 0..70u8 {
            let b: Vec<u8> = (0..n).map(|i| i.wrapping_mul(37)).collect();
            assert_eq!(b64::decode(&b64::encode(&b)).as_deref(), Some(&b[..]), "{n}");
        }
        assert_eq!(b64::encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(b64::encode(b"fo"), "Zm8=");
        assert!(b64::decode("Zm8").is_none());
        assert!(b64::decode("Z===").is_none());
    }

    #[test]
    fn object_meta_needs_both_fields() {
        let m = ObjectMeta { chain: ChainId::Repo(RepoId(DbId(4))), request: RequestId(Ulid(99)) };
        let pairs = m.to_pairs();
        assert_eq!(ObjectMeta::from_pairs(pairs.iter().map(|(k, v)| (k.as_str(), v.as_str()))), Some(m));
        assert_eq!(ObjectMeta::from_pairs([("chain", "repo:4")]), None);
        assert_eq!(ObjectMeta::from_pairs([("chain", "repo:x"), ("request", &m.request.to_string())]), None);
    }
}
