//! The fold a group's commands decide over, and the `Command` trait (`design/DESIGN.md`, Shape, "The write path").

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use graftyard_types::{ChainId, IdBlock, Timestamp};
use smallvec::SmallVec;

use crate::types::{EntryBody, Format, GroupConfig, Index, LogEntry, Next, RequestId, TierSet};
use crate::{REQUEST_RETENTION, after};

/// A stored-format record: commands and events encode themselves with `crate::encode::Enc`, so an entry's digest
/// covers them byte for byte.
pub trait Record {
    fn encode(&self, out: &mut Vec<u8>);

    fn encoded_len(&self) -> usize {
        let mut v = Vec::new();
        self.encode(&mut v);
        v.len()
    }
}

/// One writer per fold: `apply` on each member, in log order.
pub trait Fold: Clone + fmt::Debug {
    type Event: Record + Clone + fmt::Debug + PartialEq;
    /// A running view over the fold that events apply to and that is then thrown away: the scratch transaction
    /// `decide` and `verify` run on.
    type Scratch<'a>: Scratch<Event = Self::Event>
    where
        Self: 'a;

    fn scratch(&self) -> Self::Scratch<'_>;
    /// The scratch view for an entry with time `at` and tier tag `tiers`: the leader's `decide` passes the entry it is
    /// building (every tier, since its tag is the union of its commands' `tiers()`), and `verify` the entry it checks.
    /// A tiered fold overrides it: its writes stamp the entry's time, and its `check` refuses an event whose tiers the
    /// tag leaves out, which its `apply` could not write.
    fn scratch_at(&self, _at: Timestamp, _tiers: TierSet) -> Self::Scratch<'_> {
        self.scratch()
    }
    fn apply(&mut self, event: &Self::Event);
    /// A committed `EntryBody::IdBlock`: the fold's next assignments come from `block`. Folds that assign no ids
    /// ignore it.
    fn apply_id_block(&mut self, _block: IdBlock) {}
    /// Applies one committed entry, whatever its body: its index, `prev`, time, and tier tag included. By default
    /// each event of an `Events` entry in order, and an `IdBlock`'s block. A tiered fold (`graftyard_fold::apply`)
    /// needs the whole entry, so it overrides this and leaves `apply` and `apply_id_block` unused.
    fn apply_entry(&mut self, e: &LogEntry<Self::Event>) {
        match &e.body {
            EntryBody::Events(events) => {
                for ev in events {
                    self.apply(ev);
                }
            }
            EntryBody::IdBlock(b) => self.apply_id_block(*b),
            EntryBody::Config(_) | EntryBody::Noop => {}
        }
    }
    /// The tiers an event writes, which a recorded entry (`Member::record`) is tagged with. By default the core tier,
    /// the only one an untiered fold has.
    fn tiers(_event: &Self::Event) -> TierSet {
        TierSet::CORE
    }
    /// The form of an event a `request` row keeps for `REQUEST_RETENTION`: all that `Command::replay` and the host's
    /// row readers need. A fold whose events carry bulk the row does not need, such as a repository's inline pack
    /// bytes, drops it here, so a member does not hold two days of it in memory (F45). By default the whole event.
    fn row_event(event: &Self::Event) -> Self::Event {
        event.clone()
    }
}

pub trait Scratch {
    type Event;
    fn apply(&mut self, event: &Self::Event);
    /// An `EntryBody::IdBlock` stored above `applied`, which `verify` applies before it checks a later entry.
    fn apply_id_block(&mut self, _block: IdBlock) {}
    /// `verify`'s check of one event against the running state, such as a ref's or register's `expect`.
    fn check(&self, event: &Self::Event) -> Result<(), Box<str>>;
}

/// The events of a command's fold.
pub type Ev<C> = <<C as Command>::Fold as Fold>::Event;

/// A command submitted through `commit`. The design's trait also has `credential()`, which needs the identity types
/// of M1's minimal identity; it is not here yet.
pub trait Command: Record + Clone {
    type Fold: Fold;
    /// A fold of another chain, read once per entry through that chain's `fresh` by the host.
    type Dep;
    type Out: Clone + fmt::Debug + PartialEq;
    type Err: Clone + fmt::Debug + PartialEq;

    /// The stored format this command encodes to. A member whose release does not read it answers `Unsupported`.
    fn format(&self) -> Format;
    fn tiers(&self) -> TierSet;
    /// Read from the fold at `applied`, never from the command.
    fn deps(&self, fold: &Self::Fold) -> SmallVec<[ChainId; 3]>;
    /// Pure: the fold at `applied` plus the earlier commands of the same entry, and the `deps` folds.
    #[allow(clippy::type_complexity)]
    fn decide(
        &self,
        fold: &<Self::Fold as Fold>::Scratch<'_>,
        deps: &[(ChainId, Self::Dep)],
    ) -> Result<(SmallVec<[Ev<Self>; 4]>, Self::Out), Self::Err>;
    /// The output for a duplicate request, rebuilt from the events its first commit stored.
    fn replay(&self, events: &[Ev<Self>]) -> Self::Out;
}

/// A `request` table row: the entry that carried the request and its events.
#[derive(Clone, Debug, PartialEq)]
pub struct RequestRow<E> {
    pub index: Index,
    pub at: Timestamp,
    pub events: Vec<E>,
}

/// What each member applies committed entries to: the command fold, the core-tier `request` table, and the latest
/// committed `GroupConfig`. A host stores it with the same writes as the log (`Write::Apply`).
///
/// A host stores the fold, the request rows, and the config in its own tables and loads them back with `from_parts`,
/// so a restore does not replay the log.
#[derive(Clone, Debug, PartialEq)]
pub struct GroupFold<F: Fold> {
    pub fold: F,
    requests: BTreeMap<RequestId, RequestRow<F::Event>>,
    expiry: BTreeSet<(Timestamp, RequestId)>,
    config: Option<(Index, GroupConfig)>,
    /// The latest committed `EntryBody::IdBlock`, which the next one must follow (`verify`).
    id_block: Option<IdBlock>,
    applied: Index,
    /// How long rows are kept: `REQUEST_RETENTION` unless the host set it (`Member::set_request_retention`).
    retention: std::time::Duration,
}

impl<F: Fold> GroupFold<F> {
    pub fn new(fold: F) -> GroupFold<F> {
        GroupFold {
            fold,
            requests: BTreeMap::new(),
            expiry: BTreeSet::new(),
            config: None,
            id_block: None,
            applied: Index(0),
            retention: REQUEST_RETENTION,
        }
    }

    /// A group fold as its tables hold it after applying through `applied`: the command fold, the latest committed
    /// config, the latest committed id block, and the `request` rows that `apply` kept. Equal to the fold the same
    /// entries applied to.
    pub fn from_parts(
        fold: F,
        applied: Index,
        config: Option<(Index, GroupConfig)>,
        id_block: Option<IdBlock>,
        requests: impl IntoIterator<Item = (RequestId, RequestRow<F::Event>)>,
    ) -> GroupFold<F> {
        let requests: BTreeMap<RequestId, RequestRow<F::Event>> = requests.into_iter().collect();
        let expiry = requests.iter().map(|(id, r)| (r.at, *id)).collect();
        GroupFold { fold, requests, expiry, config, id_block, applied, retention: REQUEST_RETENTION }
    }

    /// How long `request` rows are kept after their entry's `at`.
    pub fn retention(&self) -> std::time::Duration {
        self.retention
    }

    pub fn set_retention(&mut self, d: std::time::Duration) {
        self.retention = d;
    }

    /// The latest committed id block.
    pub fn id_block(&self) -> Option<IdBlock> {
        self.id_block
    }

    /// Every `request` row, by request id.
    pub fn requests(&self) -> impl Iterator<Item = (&RequestId, &RequestRow<F::Event>)> {
        self.requests.iter()
    }

    pub fn applied(&self) -> Index {
        self.applied
    }

    /// The latest committed config, and its index.
    pub fn config(&self) -> Option<&(Index, GroupConfig)> {
        self.config.as_ref()
    }

    pub fn request(&self, id: RequestId) -> Option<&RequestRow<F::Event>> {
        self.requests.get(&id)
    }

    /// Applies the next committed entry. First every row more than `retention` older than the entry's `at` is dropped,
    /// so every member drops the same rows, and a host drops them with `at < entry.at - retention`.
    pub fn apply(&mut self, e: &LogEntry<F::Event>) {
        assert_eq!(e.index, self.applied.next(), "apply out of order");
        while let Some(&(at, id)) = self.expiry.first() {
            if after(at, self.retention) >= e.at {
                break;
            }
            self.expiry.pop_first();
            self.requests.remove(&id);
        }
        self.fold.apply_entry(e);
        match &e.body {
            EntryBody::Events(events) => {
                for (id, range) in &e.requests {
                    let events = events[range.start as usize..range.end as usize].iter().map(F::row_event).collect();
                    let prior = self.requests.insert(*id, RequestRow { index: e.index, at: e.at, events });
                    assert!(
                        prior.is_none(),
                        "request {id:?} applied twice, at {:?} and {:?}",
                        prior.map(|p| p.index),
                        e.index
                    );
                    self.expiry.insert((e.at, *id));
                }
            }
            EntryBody::Config(c) => self.config = Some((e.index, c.clone())),
            EntryBody::Noop => {}
            EntryBody::IdBlock(b) => self.id_block = Some(*b),
        }
        self.applied = e.index;
    }
}
