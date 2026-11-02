//! `ObjectId`, `RefName`, `RefUpdate`, and `Atomicity` (`design/SHARED.md`, "Refs and pushes").

use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// SHA-1 at launch; SHA-256 is a new variant, not a flag.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ObjectId(pub [u8; 20]);

impl ObjectId {
    /// git's null id, which a receive-pack command uses for "absent".
    pub const ZERO: ObjectId = ObjectId([0; 20]);

    /// Lower-case hex of exactly 40 characters.
    pub fn parse_hex(s: &[u8]) -> Option<ObjectId> {
        if s.len() != 40 || !s.iter().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b)) {
            return None;
        }
        let mut out = [0u8; 20];
        for (i, o) in out.iter_mut().enumerate() {
            let h = |c: u8| (c as char).to_digit(16).expect("checked") as u8;
            *o = h(s[2 * i]) << 4 | h(s[2 * i + 1]);
        }
        Some(ObjectId(out))
    }

    pub fn hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// `None` for the null id.
    pub fn non_zero(self) -> Option<ObjectId> {
        (self != ObjectId::ZERO).then_some(self)
    }
}

impl fmt::Debug for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.hex()[..10])
    }
}

/// The longest ref name accepted [guess].
pub const REF_NAME_MAX: usize = 900;

/// A full ref name that `git check-ref-format` accepts, under `refs/`, at most `REF_NAME_MAX` bytes.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RefName(Box<[u8]>);

#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum RefNameError {
    #[error("ref name over {REF_NAME_MAX} bytes")]
    TooLong,
    #[error("ref name not under refs/")]
    NotUnderRefs,
    #[error("ref name breaks git check-ref-format: {0}")]
    Format(&'static str),
}

impl RefName {
    /// git's `check_refname_format` rules for a full ref name, without `--allow-onelevel` or `--refspec-pattern`.
    pub fn parse(b: &[u8]) -> Result<RefName, RefNameError> {
        if b.len() > REF_NAME_MAX {
            return Err(RefNameError::TooLong);
        }
        if !b.starts_with(b"refs/") {
            return Err(RefNameError::NotUnderRefs);
        }
        if b == b"@" {
            return Err(RefNameError::Format("@"));
        }
        if b.ends_with(b"/") {
            return Err(RefNameError::Format("ends with /"));
        }
        if b.ends_with(b".") {
            return Err(RefNameError::Format("ends with ."));
        }
        if b.windows(2).any(|w| w == b"..") {
            return Err(RefNameError::Format(".."));
        }
        if b.windows(2).any(|w| w == b"@{") {
            return Err(RefNameError::Format("@{"));
        }
        if b.iter().any(|&c| c < 0x20 || c == 0x7f || b" ~^:?*[\\".contains(&c)) {
            return Err(RefNameError::Format("forbidden character"));
        }
        for comp in b.split(|&c| c == b'/') {
            if comp.is_empty() {
                return Err(RefNameError::Format("empty component"));
            }
            if comp.starts_with(b".") {
                return Err(RefNameError::Format("component starts with ."));
            }
            if comp.ends_with(b".lock") {
                return Err(RefNameError::Format("component ends with .lock"));
            }
        }
        Ok(RefName(b.into()))
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Lossless for every name `parse` accepts other than non-UTF-8 ones, which git allows and which render with
    /// replacement characters.
    pub fn display(&self) -> String {
        String::from_utf8_lossy(&self.0).into_owned()
    }

    pub fn starts_with(&self, prefix: &str) -> bool {
        self.0.starts_with(prefix.as_bytes())
    }
}

impl fmt::Debug for RefName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.display())
    }
}

/// `expect: None` creates. `new: None` deletes. Checked against the fresh core tier inside `decide`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefUpdate {
    pub name: RefName,
    pub expect: Option<ObjectId>,
    pub new: Option<ObjectId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Atomicity {
    AllOrNothing,
    PerRef,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_ids_parse_only_lower_hex_of_forty() {
        let h = b"0123456789abcdef0123456789abcdef01234567";
        let id = ObjectId::parse_hex(h).unwrap();
        assert_eq!(id.hex().as_bytes(), h);
        assert!(ObjectId::parse_hex(&h[..39]).is_none());
        assert!(ObjectId::parse_hex(b"0123456789ABCDEF0123456789abcdef01234567").is_none());
        assert!(ObjectId::parse_hex(b"0123456789abcdef0123456789abcdef0123456g").is_none());
        assert_eq!(ObjectId::parse_hex(&[b'0'; 40]).unwrap().non_zero(), None);
    }

    #[test]
    fn ref_names_follow_check_ref_format() {
        for ok in ["refs/heads/main", "refs/tags/v1.0", "refs/heads/a/b-c_d", "refs/heads/é", "refs/heads/x@y"] {
            assert!(RefName::parse(ok.as_bytes()).is_ok(), "{ok}");
        }
        for bad in [
            "HEAD",
            "heads/main",
            "refs/heads/",
            "refs/heads/a..b",
            "refs/heads/.hidden",
            "refs/heads/a.lock",
            "refs/heads/a//b",
            "refs/heads/a b",
            "refs/heads/a~1",
            "refs/heads/a^",
            "refs/heads/a:b",
            "refs/heads/a?",
            "refs/heads/a*",
            "refs/heads/a[",
            "refs/heads/a\\b",
            "refs/heads/a@{1}",
            "refs/heads/a.",
            "refs/heads/\x01",
        ] {
            assert!(RefName::parse(bad.as_bytes()).is_err(), "{bad:?}");
        }
        let long = format!("refs/heads/{}", "x".repeat(REF_NAME_MAX));
        assert_eq!(RefName::parse(long.as_bytes()), Err(RefNameError::TooLong));
    }
}
