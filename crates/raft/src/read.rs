//! The read path (`design/DESIGN.md`, Shape, "The read path").

use crate::types::{Index, Term};

/// Proof that a read may run on the leader: its term's `Noop` is committed and applied, and either `now <
/// lease_end - DRIFT` or a round sent after the read arrived reached a quorum. `Member::fresh` turns it into a
/// `Fresh` view, and only before the member's next `step`, so a grant cannot outlive the state it was given in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadGrant {
    pub(crate) term: Term,
    pub(crate) boot: u64,
    pub(crate) step: u64,
    pub(crate) index: Index,
    pub(crate) via_round: bool,
}

impl ReadGrant {
    /// The applied index the read sees.
    pub fn index(&self) -> Index {
        self.index
    }

    /// Whether the grant needed a read-index round because the lease had run out.
    pub fn via_round(&self) -> bool {
        self.via_round
    }
}

/// A fold that includes every entry acknowledged before the read arrived. Only the leader builds one.
pub struct Fresh<'a, F> {
    pub(crate) fold: &'a F,
    pub(crate) index: Index,
}

impl<'a, F> Fresh<'a, F> {
    pub fn fold(&self) -> &'a F {
        self.fold
    }
    pub fn index(&self) -> Index {
        self.index
    }
}

/// The fold at `applied`, served only under a customer-signed `StaleMode` flag. The flag lives in the `Release`
/// group, which is not built yet, so nothing constructs one.
pub struct Stale<'a, F> {
    #[allow(dead_code)]
    pub(crate) fold: &'a F,
    #[allow(dead_code)]
    pub(crate) index: Index,
}

impl<'a, F> Stale<'a, F> {
    pub fn fold(&self) -> &'a F {
        self.fold
    }
    pub fn index(&self) -> Index {
        self.index
    }
}

pub enum ReadView<'a, F> {
    Fresh(Fresh<'a, F>),
    Stale(Stale<'a, F>),
}

impl<'a, F> ReadView<'a, F> {
    pub fn fold(&self) -> &'a F {
        match self {
            ReadView::Fresh(f) => f.fold,
            ReadView::Stale(s) => s.fold,
        }
    }
}
