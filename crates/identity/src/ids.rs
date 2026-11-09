//! `DbId` blocks (`design/DESIGN.md`, Shape, "Chains", `DbId` and `EntryBody::IdBlock`).
//!
//! The cell's `Ids` group leases whole blocks to group leaders. A group records the block in its own log before it
//! assigns from it, and its fold keeps the next free offset, so a new leader resumes at the next free id. Ids that an
//! uncommitted entry assigned are lost with it and were never acknowledged: gaps happen, reuse never.

use graftyard_raft::{Command, Enc, Ev, Fold, Format, Record, Scratch, TierSet};
pub use graftyard_types::IdBlock;
use graftyard_types::{ChainId, DbId};
use serde::{Deserialize, Serialize};
use smallvec::{SmallVec, smallvec};

use crate::IDS_FORMAT;
use crate::credential::IdentityDep;
use crate::enc;

/// The part of a group's fold that assigns ids: the latest committed block and the next free position in it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdCursor {
    block: Option<IdBlock>,
    next: u32,
}

impl IdCursor {
    /// The id the next assignment takes, or `None` when the group holds no block or has used all of it.
    pub fn peek(&self) -> Option<DbId> {
        self.block?.id(self.next)
    }

    pub fn block(&self) -> Option<IdBlock> {
        self.block
    }

    /// Ids left in the current block.
    pub fn remaining(&self) -> u32 {
        self.block.map_or(0, |b| b.len() - self.next)
    }

    /// Whether `b` may follow the current block: a block is never taken twice and never moves backwards.
    pub fn accepts(&self, b: IdBlock) -> bool {
        self.block.is_none_or(|cur| cur.followed_by(b))
    }

    /// Applies an `IdBlock` entry. A block that `accepts` refuses changes nothing, so a duplicate lease result never
    /// rewinds the cursor.
    pub fn apply_block(&mut self, b: IdBlock) {
        if self.accepts(b) {
            *self = IdCursor { block: Some(b), next: 0 };
        }
    }

    /// Applies an event that assigned `id`.
    pub fn apply_assigned(&mut self, id: DbId) {
        if let Some(i) = self.block.and_then(|b| b.index_of(id))
            && i >= self.next
        {
            self.next = i + 1;
        }
    }
}

// ---- The `Ids` group ----

/// The cell's `Ids` fold: its cell bits and the next block it leases. Blocks start at 1, so cell 0 never assigns the
/// id 0, which SHARED excludes ("0 < id < 2^53").
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdsFold {
    pub cell: u8,
    pub next_block: u32,
}

impl IdsFold {
    pub fn new(cell: u8) -> IdsFold {
        IdsFold { cell, next_block: 1 }
    }
}

impl Default for IdsFold {
    fn default() -> IdsFold {
        IdsFold::new(0)
    }
}

/// Wire tags: postcard stores and sends the variants by position, JSON by name (`crates/git/tests/wire_tags.golden`).
/// Add a variant only at the end: never insert, remove, reorder, or rename one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum IdsEvent {
    /// `block`, a whole block of this cell, now belongs to the group of `to`.
    Leased { block: IdBlock, to: ChainId },
}

impl Record for IdsEvent {
    fn encode(&self, out: &mut Vec<u8>) {
        let mut e = Enc(out);
        match self {
            IdsEvent::Leased { block, to } => {
                e.u8(0);
                enc::db(&mut e, block.base());
                e.u32(block.len());
                enc::chain(&mut e, *to);
            }
        }
    }
}

pub struct IdsScratch<'a> {
    fold: &'a IdsFold,
    next_block: u32,
}

impl IdsScratch<'_> {
    pub fn next_block(&self) -> u32 {
        self.next_block
    }
    pub fn cell(&self) -> u8 {
        self.fold.cell
    }
}

impl Scratch for IdsScratch<'_> {
    type Event = IdsEvent;
    fn apply(&mut self, ev: &IdsEvent) {
        let IdsEvent::Leased { block, .. } = ev;
        self.next_block = self.next_block.max(block.base().block() + 1);
    }
    fn check(&self, ev: &IdsEvent) -> Result<(), Box<str>> {
        let IdsEvent::Leased { block, .. } = ev;
        let b = block.base();
        if b.cell() != self.fold.cell || b.offset() != 0 || block.len() != DbId::BLOCK_LEN {
            return Err(format!("lease of {block:?} is not a whole block of cell {}", self.fold.cell).into());
        }
        if b.block() != self.next_block {
            return Err(format!("lease of block {}, next free is {}", b.block(), self.next_block).into());
        }
        Ok(())
    }
}

impl Fold for IdsFold {
    type Event = IdsEvent;
    type Scratch<'a> = IdsScratch<'a>;
    fn scratch(&self) -> IdsScratch<'_> {
        IdsScratch { fold: self, next_block: self.next_block }
    }
    fn apply(&mut self, ev: &IdsEvent) {
        let IdsEvent::Leased { block, .. } = ev;
        self.next_block = self.next_block.max(block.base().block() + 1);
    }
}

/// Lease the next block to the group of `to`. Submitted by that group's leader, which then commits the block in its
/// own log before it assigns from it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdsCmd {
    pub to: ChainId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdsErr {
    /// Every block of the cell is leased.
    Exhausted,
}

impl Record for IdsCmd {
    fn encode(&self, out: &mut Vec<u8>) {
        let mut e = Enc(out);
        e.u8(0);
        enc::chain(&mut e, self.to);
    }
}

impl Command for IdsCmd {
    type Fold = IdsFold;
    type Dep = IdentityDep;
    type Out = IdBlock;
    type Err = IdsErr;

    fn format(&self) -> Format {
        IDS_FORMAT
    }
    fn tiers(&self) -> TierSet {
        TierSet::CORE
    }
    fn deps(&self, _fold: &IdsFold) -> SmallVec<[ChainId; 3]> {
        SmallVec::new()
    }
    fn decide(
        &self,
        fold: &IdsScratch<'_>,
        _deps: &[(ChainId, IdentityDep)],
    ) -> Result<(SmallVec<[Ev<Self>; 4]>, IdBlock), IdsErr> {
        let block = IdBlock::whole(fold.cell(), fold.next_block).ok_or(IdsErr::Exhausted)?;
        Ok((smallvec![IdsEvent::Leased { block, to: self.to }], block))
    }
    fn replay(&self, events: &[IdsEvent]) -> IdBlock {
        let IdsEvent::Leased { block, .. } = events[0];
        block
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_round_trips_and_stays_below_2_pow_53() {
        let top = DbId::pack(255, DbId::MAX_BLOCK, u16::MAX).unwrap();
        assert_eq!(top.0, (1i64 << 53) - 1);
        assert_eq!((top.cell(), top.block(), top.offset()), (255, DbId::MAX_BLOCK, u16::MAX));
        let mid = DbId::pack(7, 12_345, 6).unwrap();
        assert_eq!((mid.cell(), mid.block(), mid.offset()), (7, 12_345, 6));
        assert_eq!(DbId::pack(0, DbId::MAX_BLOCK + 1, 0), None);
    }

    #[test]
    fn id_block_refuses_empty_oversized_crossing_and_zero() {
        let base = DbId::pack(1, 9, 0).unwrap();
        assert!(IdBlock::new(base, DbId::BLOCK_LEN).is_some());
        assert!(IdBlock::new(base, DbId::BLOCK_LEN + 1).is_none());
        assert!(IdBlock::new(base, 0).is_none());
        let last = DbId::pack(1, 9, u16::MAX).unwrap();
        assert!(IdBlock::new(last, 1).is_some());
        assert!(IdBlock::new(last, 2).is_none(), "would run into block 10");
        assert!(IdBlock::new(DbId(0), 5).is_none(), "0 is not an id");
    }

    #[test]
    fn cursor_runs_to_offset_2_pow_16_minus_1_then_stops() {
        let b = IdBlock::whole(3, 41).unwrap();
        let mut c = IdCursor::default();
        assert_eq!(c.peek(), None);
        c.apply_block(b);
        let mut last = None;
        let mut n = 0u32;
        while let Some(id) = c.peek() {
            assert!(last.is_none_or(|l: DbId| id.0 == l.0 + 1));
            c.apply_assigned(id);
            last = Some(id);
            n += 1;
        }
        assert_eq!(n, DbId::BLOCK_LEN);
        let last = last.unwrap();
        assert_eq!((last.cell(), last.block(), last.offset()), (3, 41, u16::MAX));
        assert_eq!(c.remaining(), 0);
        // The same block again, or an older one, changes nothing: no rewind.
        c.apply_block(b);
        assert_eq!(c.peek(), None);
        c.apply_block(IdBlock::whole(3, 40).unwrap());
        assert_eq!(c.peek(), None);
        c.apply_block(IdBlock::whole(3, 42).unwrap());
        assert_eq!(c.peek(), DbId::pack(3, 42, 0));
    }

    #[test]
    fn ids_group_leases_each_block_once_from_block_1() {
        let mut f = IdsFold::new(2);
        let mut s = f.scratch();
        let to = ChainId::Ids;
        let (e1, b1) = IdsCmd { to }.decide(&s, &[]).unwrap();
        s.apply(&e1[0]);
        let (e2, b2) = IdsCmd { to }.decide(&s, &[]).unwrap();
        assert_eq!((b1.base().block(), b2.base().block()), (1, 2));
        assert_eq!(b1.base().cell(), 2);
        assert!(s.check(&e1[0]).is_err(), "a second lease of block 1 fails verify");
        f.apply(&e1[0]);
        f.apply(&e2[0]);
        assert_eq!(f.next_block, 3);
        f.next_block = DbId::MAX_BLOCK + 1;
        assert_eq!(IdsCmd { to }.decide(&f.scratch(), &[]), Err(IdsErr::Exhausted));
    }
}
