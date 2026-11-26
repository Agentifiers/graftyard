//! Identifiers (`design/DESIGN.md`, Shape, "Chains"; `design/SHARED.md`, "Names").

use serde::{Deserialize, Serialize};

/// `cell` (8 bits) | `block` (29) | `offset` (16), so every id stays below 2^53.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct DbId(pub i64);

impl DbId {
    pub const CELL_BITS: u32 = 8;
    pub const BLOCK_BITS: u32 = 29;
    pub const OFFSET_BITS: u32 = 16;
    /// The highest block number a cell has.
    pub const MAX_BLOCK: u32 = (1 << Self::BLOCK_BITS) - 1;
    /// Ids in one block: offsets 0 to 2^16 - 1.
    pub const BLOCK_LEN: u32 = 1 << Self::OFFSET_BITS;

    /// Packs `cell | block | offset`. `None` when `block` does not fit its 29 bits.
    pub fn pack(cell: u8, block: u32, offset: u16) -> Option<DbId> {
        (block <= Self::MAX_BLOCK).then(|| {
            DbId(
                (i64::from(cell) << (Self::BLOCK_BITS + Self::OFFSET_BITS))
                    | (i64::from(block) << Self::OFFSET_BITS)
                    | i64::from(offset),
            )
        })
    }

    /// The cell bits a managed front routes by.
    pub fn cell(self) -> u8 {
        (self.0 >> (Self::BLOCK_BITS + Self::OFFSET_BITS)) as u8
    }

    pub fn block(self) -> u32 {
        ((self.0 >> Self::OFFSET_BITS) & i64::from(Self::MAX_BLOCK)) as u32
    }

    pub fn offset(self) -> u16 {
        self.0 as u16
    }
}

/// The design's `EntryBody::IdBlock { base: DbId, len: u32 /* <= 2^16 */ }`: ids the cell's `Ids` group leased to a
/// group, recorded in that group's log before it assigns from them. Built only by `IdBlock::new`, which refuses an
/// empty block, the id 0, and one that would run past its block's last offset; decoding checks the same.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "IdBlockRaw")]
pub struct IdBlock {
    base: DbId,
    len: u32,
}

#[derive(Deserialize)]
struct IdBlockRaw {
    base: DbId,
    len: u32,
}

impl TryFrom<IdBlockRaw> for IdBlock {
    type Error = String;
    fn try_from(r: IdBlockRaw) -> Result<IdBlock, String> {
        IdBlock::new(r.base, r.len).ok_or_else(|| format!("not an id block: base {:?}, len {}", r.base, r.len))
    }
}

impl IdBlock {
    pub fn new(base: DbId, len: u32) -> Option<IdBlock> {
        let fits = len >= 1 && u64::from(base.offset()) + u64::from(len) <= u64::from(DbId::BLOCK_LEN);
        (fits && base.0 > 0).then_some(IdBlock { base, len })
    }

    /// A whole leased block: offsets 0 to 2^16 - 1.
    pub fn whole(cell: u8, block: u32) -> Option<IdBlock> {
        IdBlock::new(DbId::pack(cell, block, 0)?, DbId::BLOCK_LEN)
    }

    pub fn base(self) -> DbId {
        self.base
    }

    pub fn len(self) -> u32 {
        self.len
    }

    pub fn is_empty(self) -> bool {
        false
    }

    /// The first id after the block.
    pub fn end(self) -> i64 {
        self.base.0 + i64::from(self.len)
    }

    /// Whether `next` may follow this block in one group's log: it starts at or after this block's end.
    pub fn followed_by(self, next: IdBlock) -> bool {
        next.base.0 >= self.end()
    }

    /// The `i`th id of the block.
    pub fn id(self, i: u32) -> Option<DbId> {
        (i < self.len).then(|| DbId(self.base.0 + i64::from(i)))
    }

    /// The position of `id` in the block.
    pub fn index_of(self, id: DbId) -> Option<u32> {
        let i = id.0.checked_sub(self.base.0)?;
        (0..i64::from(self.len)).contains(&i).then_some(i as u32)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AccountId(pub DbId);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RepoId(pub DbId);

/// A network id is its root repository's id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct NetworkId(pub DbId);

impl NetworkId {
    /// The network a root repository roots: its own id (`design/SHARED.md`, "Names", `NetworkId`). A fork's network
    /// is its root's, which only the fork's `Genesis` names, so a caller that holds a fork asks for that instead.
    pub const fn of_root(repo: RepoId) -> NetworkId {
        NetworkId(repo.0)
    }

    /// The root repository of this network, whose owner's `Placement` record places the network chain and whose
    /// `RepoOwner` row homes the network (`design/DESIGN.md`, Shape, `Placement`).
    pub const fn root(self) -> RepoId {
        RepoId(self.0)
    }
}

/// SHA-256 of `release.json` (SHARED, registry).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ReleaseId(pub [u8; 32]);

/// Milliseconds since the Unix epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Timestamp(pub u64);
