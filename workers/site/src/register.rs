//! The host's register command and fold, until the repository fold exists: one string register per key with put,
//! compare-and-set, and append, like the crate's test fold (`crates/raft/tests/common/kv.rs`) and the Milestone 0
//! `Op`. Append is not idempotent, so a request applied twice shows in the value.

use std::collections::BTreeMap;

use graftyard_raft::{Command, Enc, Ev, Fold, Format, Record, Scratch, TierSet};
use graftyard_types::ChainId;
use serde::{Deserialize, Serialize};
use smallvec::{SmallVec, smallvec};

pub const REG_FORMAT: Format = Format { family: 1, version: 1 };

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reg(pub BTreeMap<String, String>);

/// Stored and sent with the crate's codec (serde); `Record::encode` is its digest form.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum RegEvent {
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

impl RegEvent {
    pub fn key(&self) -> &str {
        match self {
            RegEvent::Set { key, .. } | RegEvent::Push { key, .. } => key,
        }
    }
}

impl Record for RegEvent {
    fn encode(&self, out: &mut Vec<u8>) {
        let mut e = Enc(out);
        match self {
            RegEvent::Set { key, value, expect } => {
                e.u8(0);
                e.str(key);
                e.str(value);
                e.option(expect.as_ref(), |e, x| e.option(x.as_ref(), |e, s| e.str(s)));
            }
            RegEvent::Push { key, s } => {
                e.u8(1);
                e.str(key);
                e.str(s);
            }
        }
    }
}

fn applied(get: Option<&String>, ev: &RegEvent) -> String {
    match ev {
        RegEvent::Set { value, .. } => value.clone(),
        RegEvent::Push { s, .. } => format!("{}{s}", get.map_or("", |v| v)),
    }
}

pub struct RegScratch<'a> {
    base: &'a Reg,
    over: BTreeMap<String, String>,
}

impl RegScratch<'_> {
    pub fn get(&self, k: &str) -> Option<&String> {
        self.over.get(k).or_else(|| self.base.0.get(k))
    }
}

impl Scratch for RegScratch<'_> {
    type Event = RegEvent;
    fn apply(&mut self, ev: &RegEvent) {
        let v = applied(self.get(ev.key()), ev);
        self.over.insert(ev.key().to_owned(), v);
    }
    fn check(&self, ev: &RegEvent) -> Result<(), Box<str>> {
        match ev {
            RegEvent::Set { key, expect: Some(x), .. } if self.get(key) != x.as_ref() => {
                Err(format!("cas on {key} expected {x:?}, holds {:?}", self.get(key)).into())
            }
            _ => Ok(()),
        }
    }
}

impl Fold for Reg {
    type Event = RegEvent;
    type Scratch<'a> = RegScratch<'a>;
    fn scratch(&self) -> RegScratch<'_> {
        RegScratch { base: self, over: BTreeMap::new() }
    }
    fn apply(&mut self, ev: &RegEvent) {
        let v = applied(self.0.get(ev.key()), ev);
        self.0.insert(ev.key().to_owned(), v);
    }
}

/// The JSON form the edge sends: `{"kind":"put","key":..,"value":..}`, `cas` with `expect` (null for unset), or
/// `append`, whose `value` is appended.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum RegCmd {
    Put { key: String, value: String },
    Cas { key: String, expect: Option<String>, value: String },
    Append { key: String, value: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegErr {
    Stale,
}

impl Record for RegCmd {
    fn encode(&self, out: &mut Vec<u8>) {
        let mut e = Enc(out);
        match self {
            RegCmd::Put { key, value } => {
                e.u8(0);
                e.str(key);
                e.str(value);
            }
            RegCmd::Cas { key, expect, value } => {
                e.u8(1);
                e.str(key);
                e.option(expect.as_ref(), |e, s| e.str(s));
                e.str(value);
            }
            RegCmd::Append { key, value } => {
                e.u8(2);
                e.str(key);
                e.str(value);
            }
        }
    }
}

impl Command for RegCmd {
    type Fold = Reg;
    type Dep = Reg;
    type Out = bool;
    type Err = RegErr;

    fn format(&self) -> Format {
        REG_FORMAT
    }
    fn tiers(&self) -> TierSet {
        TierSet::CORE
    }
    fn deps(&self, _fold: &Reg) -> SmallVec<[ChainId; 3]> {
        SmallVec::new()
    }
    fn decide(
        &self,
        fold: &RegScratch<'_>,
        _deps: &[(ChainId, Reg)],
    ) -> Result<(SmallVec<[Ev<Self>; 4]>, bool), RegErr> {
        match self {
            RegCmd::Put { key, value } => {
                Ok((smallvec![RegEvent::Set { key: key.clone(), value: value.clone(), expect: None }], true))
            }
            RegCmd::Cas { key, expect, value } if fold.get(key) == expect.as_ref() => Ok((
                smallvec![RegEvent::Set { key: key.clone(), value: value.clone(), expect: Some(expect.clone()) }],
                true,
            )),
            RegCmd::Cas { .. } => Err(RegErr::Stale),
            RegCmd::Append { key, value } => {
                Ok((smallvec![RegEvent::Push { key: key.clone(), s: value.clone() }], true))
            }
        }
    }
    fn replay(&self, _events: &[RegEvent]) -> bool {
        // A refused command leaves no row, so a row means the command applied.
        true
    }
}
