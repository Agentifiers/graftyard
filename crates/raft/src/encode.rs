//! The canonical encoding that `Digest` hashes, and `bootstrap`.
//!
//! One rule for every value, so equal values have equal bytes and every voter builds the same bootstrap entry:
//!
//! - integers are fixed width, big-endian (`u8`, `u16`, `u32`, `u64`, `u128`);
//! - a byte string or `str` is its length as `u32` and then its bytes;
//! - a sequence is its count as `u32` and then each item;
//! - an `Option` is `0` for `None`, or `1` and then the value;
//! - an enum is a `u8` tag in declaration order and then its fields in declaration order;
//! - a struct is its fields in declaration order;
//! - `Timestamp` is its `u64` milliseconds, a `Duration` its whole milliseconds as `u64`;
//! - an event is its own `Record::encode` bytes, written as a byte string so the entry stays self-delimiting.
//!
//! An entry's encoding starts with the domain tag `b"graftyard/log/1"` and then the `LogEntry` fields in order,
//! `prev` included, so the digest of entry k covers the whole chain below it. `Digest` is SHA-256 of those bytes.

use graftyard_types::{ChainId, Member, MemberKind, Placement, PlacementError, PlacementRecord, Timestamp};
use sha2::{Digest as _, Sha256};
use smallvec::SmallVec;

use crate::fold::Record;
use crate::types::{Digest, EntryBody, GroupConfig, Index, LogEntry, PREV_ZERO, RAFT_FORMAT, Term, TierSet};

const DOMAIN: &[u8] = b"graftyard/log/1";

/// Appends canonical bytes. Commands and events implement `Record` with these helpers.
pub struct Enc<'a>(pub &'a mut Vec<u8>);

impl Enc<'_> {
    pub fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    pub fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_be_bytes());
    }
    pub fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_be_bytes());
    }
    pub fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_be_bytes());
    }
    pub fn u128(&mut self, v: u128) {
        self.0.extend_from_slice(&v.to_be_bytes());
    }
    pub fn bytes(&mut self, b: &[u8]) {
        self.u32(u32::try_from(b.len()).expect("a field over 4 GiB"));
        self.0.extend_from_slice(b);
    }
    pub fn str(&mut self, s: &str) {
        self.bytes(s.as_bytes());
    }
    pub fn count(&mut self, n: usize) {
        self.u32(u32::try_from(n).expect("a sequence over 2^32 items"));
    }
    pub fn option<T>(&mut self, v: Option<&T>, f: impl FnOnce(&mut Self, &T)) {
        match v {
            None => self.u8(0),
            Some(v) => {
                self.u8(1);
                f(self, v)
            }
        }
    }
}

fn member(enc: &mut Enc, m: &Member) {
    enc.u8(m.site.0);
    enc.u8(match m.kind {
        MemberKind::Full => 0,
        MemberKind::Witness => 1,
        MemberKind::Learner => 2,
    });
    enc.u8(m.slot);
    enc.u32(m.generation);
    enc.str(&m.do_name);
}

fn placement(enc: &mut Enc, p: &Placement) {
    enc.count(p.voters().len());
    for m in p.voters() {
        member(enc, m);
    }
    enc.count(p.learners().len());
    for m in p.learners() {
        member(enc, m);
    }
    enc.u8(p.home().0);
    enc.u32(p.generation());
}

fn config(enc: &mut Enc, c: &GroupConfig) {
    match c {
        GroupConfig::Normal(p) => {
            enc.u8(0);
            placement(enc, p);
        }
        GroupConfig::Degraded { placement: p, lost } => {
            enc.u8(1);
            placement(enc, p);
            enc.u8(lost.0);
        }
        GroupConfig::Joint { old, new } => {
            enc.u8(2);
            placement(enc, old);
            placement(enc, new);
        }
    }
}

/// The canonical bytes of one entry.
pub fn encode_entry<E: Record>(e: &LogEntry<E>) -> Vec<u8> {
    let mut buf = Vec::with_capacity(128);
    let mut enc = Enc(&mut buf);
    enc.0.extend_from_slice(DOMAIN);
    enc.u64(e.term.0);
    enc.u64(e.index.0);
    enc.0.extend_from_slice(&e.prev.0);
    enc.u16(e.format.family);
    enc.u16(e.format.version);
    enc.u8(e.tiers.0);
    enc.u64(e.at.0);
    enc.count(e.requests.len());
    for (id, r) in &e.requests {
        enc.u128(id.0.0);
        enc.u32(r.start);
        enc.u32(r.end);
    }
    enc.option(e.extend.as_ref(), |enc, x| {
        enc.u64(x.since.0);
        enc.u64(u64::try_from(x.by.as_millis()).unwrap_or(u64::MAX));
    });
    match &e.body {
        EntryBody::Events(events) => {
            enc.u8(0);
            enc.count(events.len());
            let mut one = Vec::new();
            for ev in events {
                one.clear();
                ev.encode(&mut one);
                enc.bytes(&one);
            }
        }
        EntryBody::Config(c) => {
            enc.u8(1);
            config(&mut enc, c);
        }
        EntryBody::Noop => enc.u8(2),
        EntryBody::IdBlock(b) => {
            enc.u8(3);
            enc.u64(b.base().0 as u64);
            enc.u32(b.len());
        }
    }
    buf
}

/// SHA-256 of the entry's canonical bytes. The next entry's `prev`.
pub fn digest<E: Record>(e: &LogEntry<E>) -> Digest {
    Digest(Sha256::digest(encode_entry(e)).into())
}

/// A group's first entry: `Config(Normal(from_record(record)))` at term 1, index 1, with a zero `prev`, no requests,
/// and `at` taken from the record, so every voter builds the same bytes and any voter may propose it.
pub fn bootstrap<E>(chain: ChainId, record: &PlacementRecord) -> Result<LogEntry<E>, PlacementError> {
    Ok(LogEntry {
        term: Term(1),
        index: Index(1),
        prev: PREV_ZERO,
        format: RAFT_FORMAT,
        tiers: TierSet::NONE,
        at: Timestamp(record.at.0),
        requests: SmallVec::new(),
        extend: None,
        body: EntryBody::Config(GroupConfig::Normal(Box::new(Placement::from_record(chain, record)?))),
    })
}
