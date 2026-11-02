//! `Acked`, built only here (`design/DESIGN.md`, Shape, "The write path", and "Invariants", acknowledgment point).
//!
//! `Acked::new` is private to this module. The member calls `settle` only on the commit-advance path, once the quorum
//! rule holds for the entry, and `replay` only for a request row, which `apply` writes only for a committed entry.
//! Both need a `QuorumProof`, which only that path builds. So a caller that renders a 2xx from `&Acked` renders it
//! only after a quorum.

use crate::quorum::QuorumProof;
use crate::types::{CommitError, Index};

/// Built only inside `outcome`, from the acks the leader counted. `report_status` and every 2xx renderer take
/// `&Acked`.
#[derive(Clone, Debug, PartialEq)]
pub struct Acked<T> {
    pub index: Index,
    pub out: T,
    quorum: QuorumProof,
}

impl<T> Acked<T> {
    fn new(index: Index, out: T, quorum: &QuorumProof) -> Acked<T> {
        Acked { index, out, quorum: quorum.clone() }
    }

    pub fn quorum(&self) -> &QuorumProof {
        &self.quorum
    }
}

/// A decided command's ticket, once its entry at `index` committed with `proof`. A refusal resolves here too, never
/// before, because it may rest on an earlier command of the same entry.
pub(crate) fn settle<O, E>(
    index: Index,
    proof: &QuorumProof,
    decided: Result<O, E>,
) -> Result<Acked<O>, CommitError<E>> {
    match decided {
        Ok(out) => Ok(Acked::new(index, out, proof)),
        Err(err) => Err(CommitError::Refused { index, err }),
    }
}

/// A duplicate request answered from its `request` row: the stored index and the replayed output. `proof` is the
/// leader's latest commit proof, which covers `index` because the row exists only once its entry applied.
pub(crate) fn replay<O>(index: Index, proof: &QuorumProof, out: O) -> Acked<O> {
    Acked::new(index, out, proof)
}
