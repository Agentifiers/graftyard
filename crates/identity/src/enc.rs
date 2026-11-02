//! Canonical encodings of the shared types the identity records carry, by `crates/raft`'s `Enc` rule (fixed-width
//! big-endian integers, length-prefixed strings, `Option` as 0 or 1 then the value, enums as a declaration-order tag).

use graftyard_raft::Enc;
use graftyard_types::{AccountId, ChainId, DbId, PlacementName, PlacementRecord, RepoId, Timestamp};

pub(crate) fn db(e: &mut Enc, id: DbId) {
    e.u64(id.0 as u64);
}

pub(crate) fn account(e: &mut Enc, id: AccountId) {
    db(e, id.0);
}

pub(crate) fn repo(e: &mut Enc, id: RepoId) {
    db(e, id.0);
}

pub(crate) fn at(e: &mut Enc, t: Timestamp) {
    e.u64(t.0);
}

pub(crate) fn chain(e: &mut Enc, c: ChainId) {
    e.str(&c.to_string());
}

pub(crate) fn placement_record(e: &mut Enc, r: &PlacementRecord) {
    e.u8(match r.name {
        PlacementName::Tri => 0,
        PlacementName::Na3 => 1,
        PlacementName::Us2w => 2,
        PlacementName::Eu2w => 3,
        PlacementName::Apac2w => 4,
        PlacementName::Solo => 5,
    });
    e.count(r.sites.len());
    for s in &r.sites {
        e.u8(s.0);
    }
    e.u8(r.home.0);
    e.u32(r.generation);
    e.option(r.moving_from.as_deref(), placement_record);
    at(e, r.at);
}
