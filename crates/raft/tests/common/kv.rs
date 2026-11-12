#![allow(dead_code)]

//! The test fold: one string register per key with put, compare-and-set, and append, like the Milestone 0 `Op`.

use std::collections::BTreeMap;

use graftyard_raft::{Command, Enc, Ev, Fold, Format, Record, Scratch, TierSet};
use graftyard_types::{AccountId, ChainId, DbId};
use smallvec::{SmallVec, smallvec};

pub const KV_FORMAT: Format = Format { family: 1, version: 1 };
/// The register an applied `EntryBody::IdBlock` writes: the block's base id. Clients never use it.
pub const ID_KEY: &str = "_id_block";
/// The register the simulator's host appends to with `Member::record`. Clients never use it.
pub const REC_KEY: &str = "_recorded";
/// The register that names the owner whose `Authz` chain a `KvCmd::PutDep` depends on, as `RepoSettings` names a
/// repository's: a `Put` to it moves the dependency by entry.
pub const OWNER_KEY: &str = "_owner";
/// A dependency fold holding this key refuses every `PutDep` decided on it: a revoked credential.
pub const REVOKED_KEY: &str = "revoked";
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
    /// An admission test's write (`KvCmd::Write`): `actor` and `push` are what admission counts, and `pack` the inline
    /// pack bytes the entry carries.
    Write {
        key: String,
        value: String,
        actor: Option<u64>,
        push: bool,
        pack: usize,
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
            KvEvent::Write { key, value, actor, push, pack } => {
                e.u8(2);
                e.str(key);
                e.str(value);
                e.option(actor.as_ref(), |e, a| e.u64(*a));
                e.u8(u8::from(*push));
                e.bytes(&vec![0; *pack]);
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

    /// The fold at `applied`, without the earlier commands of the entry.
    pub fn base(&self) -> &Kv {
        self.base
    }
}

/// The chain a `PutDep` depends on, read from the fold at `applied`: the owner `OWNER_KEY` names, else one of three
/// owners by the count of the simulator host's records, so recorded entries move it too.
pub fn owner_chain(fold: &Kv) -> ChainId {
    let n = match fold.0.get(OWNER_KEY) {
        Some(v) => v.parse().unwrap_or(0),
        None => fold.0.get(REC_KEY).map_or(0, |v| v.matches(',').count() as i64 % 3),
    };
    ChainId::Authz(AccountId(DbId(n)))
}

fn apply_to(get: Option<&String>, ev: &KvEvent) -> String {
    match ev {
        KvEvent::Set { value, .. } => value.clone(),
        KvEvent::Push { s, .. } => format!("{}{s}", get.map_or("", |v| v)),
        KvEvent::Write { value, .. } => value.clone(),
    }
}

fn key(ev: &KvEvent) -> &str {
    match ev {
        KvEvent::Set { key, .. } | KvEvent::Push { key, .. } | KvEvent::Write { key, .. } => key,
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
    fn write_mark(ev: &KvEvent) -> Option<graftyard_raft::admit::WriteMark> {
        match ev {
            KvEvent::Write { actor, push, .. } => Some(graftyard_raft::admit::WriteMark {
                actor: actor.map(graftyard_raft::admit::ActorKey),
                push: *push,
            }),
            _ => None,
        }
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
    /// A put that depends on one other chain, named by the fold (`owner_chain`): `decide` takes exactly that chain's
    /// fold, refusing `NoDep` for any other set and `Revoked` when the fold holds `REVOKED_KEY`.
    PutDep {
        key: String,
        value: String,
    },
    /// A write as admission sees it: a push is a compare-and-set of `key` from `expect` (refused `Stale` otherwise),
    /// any other write a put. `pack` bytes ride inline.
    Write {
        key: String,
        expect: Option<String>,
        value: String,
        actor: Option<u64>,
        push: bool,
        pack: usize,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KvErr {
    Stale,
    /// `decide` was not given exactly the dependency `deps()` names on the fold at `applied`.
    NoDep,
    Revoked,
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
            KvCmd::Write { key, expect, value, actor, push, pack } => {
                e.u8(5);
                e.str(key);
                opt(&mut e, expect.as_ref());
                e.str(value);
                e.option(actor.as_ref(), |e, a| e.u64(*a));
                e.u8(u8::from(*push));
                e.bytes(&vec![0; *pack]);
            }
            KvCmd::PutDep { key, value } => {
                e.u8(6);
                e.str(key);
                e.str(value);
            }
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
    fn deps(&self, fold: &Kv) -> SmallVec<[ChainId; 3]> {
        match self {
            KvCmd::PutDep { .. } => smallvec![owner_chain(fold)],
            _ => SmallVec::new(),
        }
    }
    fn decide(&self, fold: &KvScratch<'_>, deps: &[(ChainId, Kv)]) -> Result<(SmallVec<[Ev<Self>; 4]>, bool), KvErr> {
        match self {
            KvCmd::PutDep { key, value } => match deps {
                [(c, d)] if *c == owner_chain(fold.base()) => {
                    if d.0.contains_key(REVOKED_KEY) {
                        return Err(KvErr::Revoked);
                    }
                    Ok((smallvec![KvEvent::Set { key: key.clone(), value: value.clone(), expect: None }], true))
                }
                _ => Err(KvErr::NoDep),
            },
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
            KvCmd::Write { key, expect, push: true, .. } if fold.get(key) != expect.as_ref() => Err(KvErr::Stale),
            KvCmd::Write { key, value, actor, push, pack, .. } => Ok((
                smallvec![KvEvent::Write {
                    key: key.clone(),
                    value: value.clone(),
                    actor: *actor,
                    push: *push,
                    pack: *pack
                }],
                true,
            )),
        }
    }
    fn replay(&self, _events: &[KvEvent]) -> bool {
        // A refused command leaves no row, so a row means the command applied.
        true
    }
}
