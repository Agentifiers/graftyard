#![allow(dead_code)]

//! The test fold: one string register per key with put, compare-and-set, and append, like the Milestone 0 `Op`.

use std::collections::BTreeMap;

use graftyard_raft::{Command, Enc, Ev, Fold, Format, Record, Scratch, TierSet};
use graftyard_types::ChainId;
use smallvec::{SmallVec, smallvec};

pub const KV_FORMAT: Format = Format { family: 1, version: 1 };
/// The register an applied `EntryBody::IdBlock` writes: the block's base id. Clients never use it.
pub const ID_KEY: &str = "_id_block";
/// The register the simulator's host appends to with `Member::record`. Clients never use it.
pub const REC_KEY: &str = "_recorded";
/// A format no member of these tests reads.
pub const FUTURE_FORMAT: Format = Format { family: 1, version: 2 };

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Kv(pub BTreeMap<String, String>);

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum KvEvent {
    /// `expect` is set for a compare-and-set, and `verify` checks it against the running state.
    Set {
        key: String,
        value: String,
        expect: Option<Option<String>>,
    },
    Push {
        key: String,
        s: String,
    },
}

fn opt(enc: &mut Enc, v: Option<&String>) {
    enc.option(v, |e, s| e.str(s));
}

impl Record for KvEvent {
    fn encode(&self, out: &mut Vec<u8>) {
        let mut e = Enc(out);
        match self {
            KvEvent::Set { key, value, expect } => {
                e.u8(0);
                e.str(key);
                e.str(value);
                e.option(expect.as_ref(), |e, x| opt(e, x.as_ref()));
            }
            KvEvent::Push { key, s } => {
                e.u8(1);
                e.str(key);
                e.str(s);
            }
        }
    }
}

pub struct KvScratch<'a> {
    base: &'a Kv,
    over: BTreeMap<String, String>,
}

impl KvScratch<'_> {
    pub fn get(&self, k: &str) -> Option<&String> {
        self.over.get(k).or_else(|| self.base.0.get(k))
    }
}

fn apply_to(get: Option<&String>, ev: &KvEvent) -> String {
    match ev {
        KvEvent::Set { value, .. } => value.clone(),
        KvEvent::Push { s, .. } => format!("{}{s}", get.map_or("", |v| v)),
    }
}

fn key(ev: &KvEvent) -> &str {
    match ev {
        KvEvent::Set { key, .. } | KvEvent::Push { key, .. } => key,
    }
}

impl Scratch for KvScratch<'_> {
    type Event = KvEvent;
    fn apply(&mut self, ev: &KvEvent) {
        let v = apply_to(self.get(key(ev)), ev);
        self.over.insert(key(ev).to_owned(), v);
    }
    fn apply_id_block(&mut self, b: graftyard_raft::IdBlock) {
        self.over.insert(ID_KEY.to_owned(), b.base().0.to_string());
    }
    fn check(&self, ev: &KvEvent) -> Result<(), Box<str>> {
        match ev {
            KvEvent::Set { key, expect: Some(x), .. } if self.get(key) != x.as_ref() => {
                Err(format!("cas on {key} expected {x:?}, holds {:?}", self.get(key)).into())
            }
            _ => Ok(()),
        }
    }
}

impl Fold for Kv {
    type Event = KvEvent;
    type Scratch<'a> = KvScratch<'a>;
    fn scratch(&self) -> KvScratch<'_> {
        KvScratch { base: self, over: BTreeMap::new() }
    }
    fn apply(&mut self, ev: &KvEvent) {
        let v = apply_to(self.0.get(key(ev)), ev);
        self.0.insert(key(ev).to_owned(), v);
    }
    fn apply_id_block(&mut self, b: graftyard_raft::IdBlock) {
        self.0.insert(ID_KEY.to_owned(), b.base().0.to_string());
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum KvCmd {
    Put {
        key: String,
        value: String,
    },
    Cas {
        key: String,
        expect: Option<String>,
        value: String,
    },
    /// Not idempotent, so a double apply shows in the value.
    Append {
        key: String,
        s: String,
    },
    /// A command of `bytes` bytes, for `COMMAND_MAX`.
    Big {
        bytes: usize,
    },
    /// A command in a format no member reads.
    Future,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KvErr {
    Stale,
}

impl Record for KvCmd {
    fn encode(&self, out: &mut Vec<u8>) {
        let mut e = Enc(out);
        match self {
            KvCmd::Put { key, value } => {
                e.u8(0);
                e.str(key);
                e.str(value);
            }
            KvCmd::Cas { key, expect, value } => {
                e.u8(1);
                e.str(key);
                opt(&mut e, expect.as_ref());
                e.str(value);
            }
            KvCmd::Append { key, s } => {
                e.u8(2);
                e.str(key);
                e.str(s);
            }
            KvCmd::Big { bytes } => {
                e.u8(3);
                e.bytes(&vec![0; *bytes]);
            }
            KvCmd::Future => e.u8(4),
        }
    }
}

impl Command for KvCmd {
    type Fold = Kv;
    type Dep = Kv;
    type Out = bool;
    type Err = KvErr;

    fn format(&self) -> Format {
        if matches!(self, KvCmd::Future) { FUTURE_FORMAT } else { KV_FORMAT }
    }
    fn tiers(&self) -> TierSet {
        TierSet::CORE
    }
    fn deps(&self, _fold: &Kv) -> SmallVec<[ChainId; 3]> {
        SmallVec::new()
    }
    fn decide(&self, fold: &KvScratch<'_>, _deps: &[(ChainId, Kv)]) -> Result<(SmallVec<[Ev<Self>; 4]>, bool), KvErr> {
        match self {
            KvCmd::Put { key, value } => {
                Ok((smallvec![KvEvent::Set { key: key.clone(), value: value.clone(), expect: None }], true))
            }
            KvCmd::Cas { key, expect, value } if fold.get(key) == expect.as_ref() => Ok((
                smallvec![KvEvent::Set { key: key.clone(), value: value.clone(), expect: Some(expect.clone()) }],
                true,
            )),
            KvCmd::Cas { .. } => Err(KvErr::Stale),
            KvCmd::Append { key, s } => Ok((smallvec![KvEvent::Push { key: key.clone(), s: s.clone() }], true)),
            KvCmd::Big { .. } | KvCmd::Future => Ok((SmallVec::new(), true)),
        }
    }
    fn replay(&self, _events: &[KvEvent]) -> bool {
        // A refused command leaves no row, so a row means the command applied.
        true
    }
}
