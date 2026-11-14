//! Runner and job labels (SHARED, "Label matching"): lowercased at registration and job creation, and a job may run
//! on a runner whose labels include all of the job's.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// A label's longest form [guess: GitHub documents none; 256 bounds an entry].
pub const LABEL_MAX: usize = 256;
/// Labels per set [guess].
pub const LABELS_MAX: usize = 32;

/// A sorted, lowercased, de-duplicated set of labels [unverified: that GitHub matches labels case-insensitively].
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct LabelSet(BTreeSet<Box<str>>);

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum LabelError {
    #[error("a label is empty or longer than {LABEL_MAX} bytes")]
    Bad,
    #[error("more than {LABELS_MAX} labels")]
    TooMany,
}

impl LabelSet {
    pub fn parse<I, S>(labels: I) -> Result<LabelSet, LabelError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut out = BTreeSet::new();
        for l in labels {
            let l = l.as_ref().trim();
            if l.is_empty() || l.len() > LABEL_MAX {
                return Err(LabelError::Bad);
            }
            out.insert(l.to_lowercase().into_boxed_str());
        }
        if out.len() > LABELS_MAX {
            return Err(LabelError::TooMany);
        }
        Ok(LabelSet(out))
    }

    /// Whether a runner with labels `runner` may take a job that asks for `self`.
    pub fn fits(&self, runner: &LabelSet) -> bool {
        self.0.is_subset(&runner.0)
    }

    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(|s| &**s)
    }

    /// The set's hash, which the broker keys its table on (SHARED).
    pub fn hash(&self) -> [u8; 32] {
        let mut h = Sha256::new();
        for l in &self.0 {
            h.update(l.as_bytes());
            h.update([0]);
        }
        h.finalize().into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_match_by_subset_and_ignore_case() {
        let job = LabelSet::parse(["Self-Hosted", "linux"]).unwrap();
        let runner = LabelSet::parse(["self-hosted", "LINUX", "x64"]).unwrap();
        assert!(job.fits(&runner));
        assert!(!runner.fits(&job), "a runner's extra label is not a job's");
        assert!(!LabelSet::parse(["gpu"]).unwrap().fits(&runner));
        assert!(LabelSet::default().fits(&runner), "a job with no labels fits any runner");
        assert_eq!(job.hash(), LabelSet::parse(["LINUX", "self-hosted", "linux"]).unwrap().hash());
        assert_ne!(LabelSet::parse(["ab", "c"]).unwrap().hash(), LabelSet::parse(["a", "bc"]).unwrap().hash());
        assert_eq!(LabelSet::parse([""]), Err(LabelError::Bad));
    }
}
