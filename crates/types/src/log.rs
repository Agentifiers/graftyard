//! Log positions and request ids, shared by the Raft core and the pack store (`design/DESIGN.md`, Shape, "Chains" and
//! "The write path"). `Ulid` is the design's id type for `RequestId` and `PackId`.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::ids::Timestamp;

/// A chain's log index. Index 1 is the bootstrap entry.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Index(pub u64);

/// A fold tier (`design/SHARED.md`, "Folds", `tier_of`). Snapshots are kept per tier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Core,
    Metadata,
}

/// A SHA-256: an entry's digest, a range's content digest in `R2Key::Archive` and `R2Key::Snapshot`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Digest(pub [u8; 32]);

impl Digest {
    pub fn hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Lower-case hex of exactly 64 characters.
    pub fn parse_hex(s: &str) -> Option<Digest> {
        if s.len() != 64 || !s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
            return None;
        }
        let mut out = [0u8; 32];
        for (i, o) in out.iter_mut().enumerate() {
            *o = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()?;
        }
        Some(Digest(out))
    }
}

impl fmt::Debug for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.hex()[..12])
    }
}

/// 48 bits of milliseconds since the Unix epoch, then 80 random bits, written as 26 Crockford base-32 characters.
/// Serialized as its text form, so one id has one spelling in JSON, R2 keys, and R2 custom metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct Ulid(pub u128);

const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

impl Ulid {
    /// `rand` supplies the low 80 bits. The host draws it; this crate has no randomness of its own.
    pub fn new(at: Timestamp, rand: [u8; 10]) -> Ulid {
        let mut low = [0u8; 16];
        low[6..].copy_from_slice(&rand);
        Ulid(((u128::from(at.0) & 0xffff_ffff_ffff) << 80) | u128::from_be_bytes(low))
    }

    /// The time the id was minted at.
    pub fn time(self) -> Timestamp {
        Timestamp((self.0 >> 80) as u64)
    }
}

impl fmt::Display for Ulid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut out = [0u8; 26];
        for (i, c) in out.iter_mut().enumerate() {
            let shift = 5 * (25 - i);
            *c = CROCKFORD[((self.0 >> shift) & 31) as usize];
        }
        f.write_str(std::str::from_utf8(&out).expect("ascii"))
    }
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[error("not a ULID: {0:?}")]
pub struct ParseUlidError(pub String);

impl FromStr for Ulid {
    type Err = ParseUlidError;

    /// Upper-case canonical form only, so an id has exactly one spelling inside keys.
    fn from_str(s: &str) -> Result<Ulid, ParseUlidError> {
        let bad = || ParseUlidError(s.to_owned());
        if s.len() != 26 || s.as_bytes()[0] > b'7' {
            return Err(bad());
        }
        let mut v: u128 = 0;
        for b in s.bytes() {
            let d = CROCKFORD.iter().position(|c| *c == b).ok_or_else(bad)?;
            v = (v << 5) | d as u128;
        }
        Ok(Ulid(v))
    }
}

impl From<Ulid> for String {
    fn from(u: Ulid) -> String {
        u.to_string()
    }
}

impl TryFrom<String> for Ulid {
    type Error = ParseUlidError;
    fn try_from(s: String) -> Result<Ulid, ParseUlidError> {
        s.parse()
    }
}

/// Minted by the edge, one per client request, and reused for every re-send of that request's command.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RequestId(pub Ulid);

impl fmt::Display for RequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for RequestId {
    type Err = ParseUlidError;
    fn from_str(s: &str) -> Result<RequestId, ParseUlidError> {
        s.parse().map(RequestId)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_hex_is_lower_case_64_characters_only() {
        let d = Digest([0xa5; 32]);
        assert_eq!(Digest::parse_hex(&d.hex()), Some(d));
        for bad in [d.hex().to_uppercase(), d.hex()[1..].to_owned(), format!("{}0", d.hex()), "g".repeat(64)] {
            assert_eq!(Digest::parse_hex(&bad), None, "{bad}");
        }
    }

    #[test]
    fn ulid_text_round_trips_keeps_time_and_rejects_neighbours() {
        let at = Timestamp(1_793_491_200_000);
        let u = Ulid::new(at, [0xff; 10]);
        assert_eq!(u.time(), at);
        let s = u.to_string();
        assert_eq!(s.len(), 26);
        assert_eq!(s.parse::<Ulid>(), Ok(u));
        assert_eq!(serde_json::to_string(&RequestId(u)).unwrap(), format!("\"{s}\""));
        assert_eq!(Ulid(0).to_string(), "0".repeat(26));
        assert_eq!(Ulid(u128::MAX).to_string().parse::<Ulid>(), Ok(Ulid(u128::MAX)));
        // Lower case, I/L/O/U, a 27th character, and an overflowing first digit are all refused.
        for bad in [s.to_lowercase(), format!("{s}0"), s[1..].to_owned(), format!("8{}", &s[1..]), "I".repeat(26)] {
            assert!(bad.parse::<Ulid>().is_err(), "{bad}");
        }
        // Ids sort by time first, so a key listing sorts packs by their minting time.
        assert!(Ulid::new(Timestamp(1), [0xff; 10]) < Ulid::new(Timestamp(2), [0; 10]));
        assert!(Ulid::new(Timestamp(1), [0xff; 10]).to_string() < Ulid::new(Timestamp(2), [0; 10]).to_string());
    }
}
