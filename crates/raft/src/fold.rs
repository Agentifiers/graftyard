//! The fold a group's commands decide over, and the `Command` trait (`design/DESIGN.md`, Shape, "The write path").

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use graftyard_types::{ChainId, IdBlock, Timestamp};
use smallvec::SmallVec;

use crate::REQUEST_RETENTION;
use crate::types::{EntryBody, Format, GroupConfig, Index, LogEntry, Next, RequestId, TierSet};

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
    type Event: Record + Clone + fmt::Debug + PartialEq + 'static;
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
    /// The write an event records for admission: whose write it was, for DEMOTE_PER_MIN, and whether it moved a
    /// ref, for `hot` (`crate::admit`). By default none, so a fold that marks nothing never demotes or turns hot.
    fn write_mark(_event: &Self::Event) -> Option<crate::admit::WriteMark> {
        None
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

/// Where a group fold keeps its `request` table. `MemRequests` holds every row in memory, as the crate's reference
/// storage does. A host whose rows outlive what an isolate can hold keeps them on its own storage and answers point
/// lookups from it (`workers/site`, `RequestSql`): two days of rows at the gate's 300 commands/s are 52 M rows, and a
/// member that loaded 90,048 of them into memory at restore passed its isolate's limit and served nothing more
/// (`tests/m2/results/m2-wp1.md`, "The restore defect").
pub trait RequestStore<E>: fmt::Debug {
    /// The row of `id`, unless its entry is more than retention older than the last applied one (`expire`).
    fn get(&self, id: RequestId) -> Option<RequestRow<E>>;
    /// Adds a row. False if `id` already has one, which is a double apply.
    fn put(&mut self, id: RequestId, row: RequestRow<E>) -> bool;
    /// Drops every row whose `at` is before `cut`.
    fn expire(&mut self, cut: Timestamp);
    /// Every row, by request id: for checks of a short log only, since a host store reads them all.
    fn rows(&self) -> Vec<(RequestId, RequestRow<E>)>;
    fn clone_box(&self) -> Box<dyn RequestStore<E>>;
}

/// Every row in memory, with the expiry order.
#[derive(Clone, Debug)]
pub struct MemRequests<E> {
    rows: BTreeMap<RequestId, RequestRow<E>>,
    expiry: BTreeSet<(Timestamp, RequestId)>,
}

impl<E> Default for MemRequests<E> {
    fn default() -> Self {
        MemRequests { rows: BTreeMap::new(), expiry: BTreeSet::new() }
    }
}

impl<E> FromIterator<(RequestId, RequestRow<E>)> for MemRequests<E> {
    fn from_iter<I: IntoIterator<Item = (RequestId, RequestRow<E>)>>(it: I) -> Self {
        let rows: BTreeMap<RequestId, RequestRow<E>> = it.into_iter().collect();
        let expiry = rows.iter().map(|(id, r)| (r.at, *id)).collect();
        MemRequests { rows, expiry }
    }
}

impl<E: Clone + fmt::Debug + 'static> RequestStore<E> for MemRequests<E> {
    fn get(&self, id: RequestId) -> Option<RequestRow<E>> {
        self.rows.get(&id).cloned()
    }
    fn put(&mut self, id: RequestId, row: RequestRow<E>) -> bool {
        if self.rows.contains_key(&id) {
            return false;
        }
        self.expiry.insert((row.at, id));
        self.rows.insert(id, row);
        true
    }
    fn expire(&mut self, cut: Timestamp) {
        while let Some(&(at, id)) = self.expiry.first() {
            if at >= cut {
                break;
            }
            self.expiry.pop_first();
            self.rows.remove(&id);
        }
    }
    fn rows(&self) -> Vec<(RequestId, RequestRow<E>)> {
        self.rows.iter().map(|(id, r)| (*id, r.clone())).collect()
    }
    fn clone_box(&self) -> Box<dyn RequestStore<E>> {
        Box::new(self.clone())
    }
}

/// What each member applies committed entries to: the command fold, the core-tier `request` table, and the latest
/// committed `GroupConfig`. A host stores it with the same writes as the log (`Write::Apply`).
///
/// A host stores the fold, the request rows, and the config in its own tables and loads them back with `from_parts`,
/// so a restore does not replay the log.
#[derive(Debug)]
pub struct GroupFold<F: Fold> {
    pub fold: F,
    requests: Box<dyn RequestStore<F::Event>>,
    config: Option<(Index, GroupConfig)>,
    /// The latest committed `EntryBody::IdBlock`, which the next one must follow (`verify`).
    id_block: Option<IdBlock>,
    applied: Index,
    /// How long rows are kept: `REQUEST_RETENTION` unless the host set it (`Member::set_request_retention`).
    retention: std::time::Duration,
    /// The committed writes of the last `DEMOTE_WINDOW`, by entry time, from `Fold::write_mark`: the per-actor counts
    /// DEMOTE_PER_MIN reads and the push count `hot` reads, so a new leader reads what its predecessor committed.
    recent: crate::admit::Recent,
}

impl<F: Fold> Clone for GroupFold<F> {
    fn clone(&self) -> Self {
        GroupFold {
            fold: self.fold.clone(),
            requests: self.requests.clone_box(),
            config: self.config.clone(),
            id_block: self.id_block,
            applied: self.applied,
            retention: self.retention,
            recent: self.recent.clone(),
        }
    }
}

/// Equal folds, configs, and request rows. Reads every row, so only for checks of a short log.
impl<F: Fold + PartialEq> PartialEq for GroupFold<F> {
    fn eq(&self, o: &Self) -> bool {
        self.fold == o.fold
            && self.config == o.config
            && self.id_block == o.id_block
            && self.applied == o.applied
            && self.retention == o.retention
            && self.requests.rows() == o.requests.rows()
    }
}

impl<F: Fold> GroupFold<F> {
    /// A fold with no entries applied, its `request` table in memory.
    pub fn new(fold: F) -> GroupFold<F> {
        GroupFold::from_parts(fold, Index(0), None, None, Box::new(MemRequests::default()))
    }

    /// A group fold as its tables hold it after applying through `applied`: the command fold, the latest committed
    /// config, the latest committed id block, and the `request` table that `apply` kept. Equal to the fold the same
    /// entries applied to.
    pub fn from_parts(
        fold: F,
        applied: Index,
        config: Option<(Index, GroupConfig)>,
        id_block: Option<IdBlock>,
        requests: Box<dyn RequestStore<F::Event>>,
    ) -> GroupFold<F> {
        GroupFold {
            fold,
            requests,
            config,
            id_block,
            applied,
            retention: REQUEST_RETENTION,
            recent: crate::admit::Recent::default(),
        }
    }

    /// Committed writes by `actor` with entry times in `[from, before)`, as far as this fold has seen them.
    pub fn actor_writes(&self, actor: crate::admit::ActorKey, from: Timestamp, before: Timestamp) -> u32 {
        self.recent.actor_writes(actor, from, before)
    }

    /// Committed pushes with entry times in `[from, before)`.
    pub fn pushes(&self, from: Timestamp, before: Timestamp) -> u32 {
        self.recent.pushes(from, before)
    }

    /// Notes an entry's writes in the admission window without applying it: what `Member::restore` does for the
    /// applied entries its stored window holds, since the window is not in the host's tables.
    pub(crate) fn note_writes(&mut self, e: &LogEntry<F::Event>) {
        if e.index <= self.recent.through {
            return;
        }
        self.recent.through = e.index;
        let cut = Timestamp(e.at.0.saturating_sub(crate::admit::DEMOTE_WINDOW.as_millis() as u64));
        self.recent.expire(cut);
        if let EntryBody::Events(events) = &e.body {
            for ev in events {
                if let Some(m) = F::write_mark(ev) {
                    self.recent.note(e.at, m);
                }
            }
        }
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

    /// Every `request` row, by request id. Reads them all from the store, so only for checks of a short log.
    pub fn requests(&self) -> Vec<(RequestId, RequestRow<F::Event>)> {
        self.requests.rows()
    }

    pub fn applied(&self) -> Index {
        self.applied
    }

    /// The latest committed config, and its index.
    pub fn config(&self) -> Option<&(Index, GroupConfig)> {
        self.config.as_ref()
    }

    pub fn request(&self, id: RequestId) -> Option<RequestRow<F::Event>> {
        self.requests.get(id)
    }

    /// Applies the next committed entry. First every row more than `retention` older than the entry's `at` is dropped,
    /// so every member drops the same rows, and a host drops them with `at < entry.at - retention`.
    pub fn apply(&mut self, e: &LogEntry<F::Event>) {
        assert_eq!(e.index, self.applied.next(), "apply out of order");
        self.requests.expire(Timestamp(e.at.0.saturating_sub(self.retention.as_millis() as u64)));
        self.fold.apply_entry(e);
        self.note_writes(e);
        match &e.body {
            EntryBody::Events(events) => {
                for (id, range) in &e.requests {
                    let events = events[range.start as usize..range.end as usize].iter().map(F::row_event).collect();
                    let row = RequestRow { index: e.index, at: e.at, events };
                    if !self.requests.put(*id, row) {
                        panic!(
                            "request {id:?} applied twice, at {:?} and {:?}",
                            self.requests.get(*id).map(|p| p.index),
                            e.index
                        );
                    }
                }
            }
            EntryBody::Config(c) => self.config = Some((e.index, c.clone())),
            EntryBody::Noop => {}
            EntryBody::IdBlock(b) => self.id_block = Some(*b),
        }
        self.applied = e.index;
    }
}
