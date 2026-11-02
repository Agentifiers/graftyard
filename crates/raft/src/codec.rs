//! The one storage and wire encoding of the crate's types: `LogEntry`, `MemberState`, `Envelope` and every `Message`,
//! `GroupConfig`, and anything else that derives serde here. Hosts store log rows and the state row with it and send
//! member-to-member messages with it, so a field added to a crate type is carried with no host change.
//!
//! The bytes are one version byte, `CODEC_VERSION`, and then postcard (varint integers, length-prefixed sequences,
//! enum variants by index). A host's event type is encoded by its own `Serialize`, so it must derive serde too.
//!
//! This is not the digest's encoding. `Digest` stays SHA-256 of `encode::encode_entry`, the canonical form that every
//! voter rebuilds byte for byte, and a host that decodes a stored entry rehashes it to check it against the stored
//! digest.

use std::fmt;

use serde::Serialize;
use serde::de::DeserializeOwned;

/// Bumped when a stored or sent shape changes incompatibly. A decoder refuses any other version.
pub const CODEC_VERSION: u8 = 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodecError(pub String);

impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CodecError {}

pub fn to_bytes<T: Serialize + ?Sized>(v: &T) -> Vec<u8> {
    let mut out = vec![CODEC_VERSION];
    // Postcard fails only on types it cannot represent (maps with unknown length, `deserialize_any`); none derive
    // here, so a failure is a programming error.
    out.extend(postcard::to_allocvec(v).expect("crate types encode"));
    out
}

/// Decodes bytes `to_bytes` wrote, refusing another version and trailing bytes.
pub fn from_bytes<T: DeserializeOwned>(b: &[u8]) -> Result<T, CodecError> {
    match b.split_first() {
        Some((&CODEC_VERSION, rest)) => {
            let (v, tail) = postcard::take_from_bytes(rest).map_err(|e| CodecError(e.to_string()))?;
            if tail.is_empty() { Ok(v) } else { Err(CodecError(format!("{} trailing bytes", tail.len()))) }
        }
        Some((v, _)) => Err(CodecError(format!("codec version {v}, this release reads {CODEC_VERSION}"))),
        None => Err(CodecError("empty record".into())),
    }
}
