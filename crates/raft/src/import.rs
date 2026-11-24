//! The lifeboat's importer (`design/DESIGN.md`, Shape, "Lifeboat feed"; "Install operations", Lifeboat;
//! `decisions/DECISIONS.md`, "`lifeboat-learner`"): it feeds one chain's committed rows, from the R2 feed or the export
//! endpoints, into a learner that has no leader, so the member's own `verify`, apply, and fold build the copy.
//!
//! - **Its own address.** The importer steps rows into the member as an AppendEntries from `IMPORTER`, an address no
//!   placement names (sites are below 64, learners from `LEARNER_ADDR`), with `commit` at the last row, since every
//!   row it sends is committed. The member answers in its `AppendRes`, which the importer reads and drops.
//! - **The chain check.** The member checks a row's `prev` against its own digest only above its commit; at or below,
//!   a mismatch is a `debug_assert`, so a release build would answer "not matched" and wait. The importer checks every
//!   row itself first: the first row's `prev` against the digest of the last row it holds, each later row's against
//!   the one before, and each row's index. A mismatch halts the copy (`Halt::Chain`), never a skip.
//! - **Undecodable rows.** A row in a format the member's release does not read is refused by `verify` at its index,
//!   and refused again on every retry: the copy halts there (`Halt::Refused`) and its commit stays below it.
//! - **Snapshots.** A range archived out of every source's log comes as an `InstallSnapshot` from the importer
//!   (`install`); the host loads the objects it copied, calls `Member::install`, and the importer goes on from
//!   `through`.
//! - **The record.** No export object carries a chain's `PlacementRecord`, which the member's config needs. `record_of`
//!   rebuilds it from entry 1, the bootstrap `Config`, and keeps it only if `bootstrap` of it is entry 1's bytes.

use graftyard_types::{ChainId, PlacementName, PlacementRecord, SiteId, Timestamp};

use crate::codec;
use crate::encode::{bootstrap, digest};
use crate::fold::{Command, Ev};
use crate::member::{AppendReq, InstallSnapshot, InstallSource, Member, Message, Ready, Refusal};
use crate::types::{Digest, EntryBody, GroupConfig, Index, LogEntry, Next, PREV_ZERO, Term};

/// The importer's address in the messages it steps into its member: above every learner address, so no placement
/// can name it.
pub const IMPORTER: SiteId = SiteId(250);

/// Why the copy stopped. Each one alarms, and the host steps nothing more into the chain until a person acts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Halt {
    /// A row's bytes do not decode as this release's `LogEntry`.
    Undecodable { index: Option<Index>, why: String },
    /// A row's index or `prev` does not follow the row before it, or the last one the member holds.
    Chain { index: Index, why: String },
    /// The member's `verify` refused the row: a format this release does not read, or an event its fold refuses.
    Refused(Refusal),
}

/// What one `feed` did: the member's `Ready`, which the host persists before anything else (an idle one when no row
/// was new), and why the copy stopped, if it did.
pub struct Fed<C: Command> {
    pub ready: Ready<C, ()>,
    pub halt: Option<Halt>,
}

/// One chain's importer: the last row the member holds, by index and digest, and the round counter its appends carry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Importer {
    held: Index,
    digest: Digest,
    round: u64,
}

impl Importer {
    /// An importer for a member that holds nothing yet.
    pub fn new() -> Importer {
        Importer { held: Index(0), digest: PREV_ZERO, round: 0 }
    }

    /// An importer for a member restored from storage: it goes on from the member's last entry.
    pub fn resume<C: Command>(m: &Member<C>) -> Importer {
        let held = m.last_index();
        Importer { held, digest: m.digest_of(held).unwrap_or(PREV_ZERO), round: 0 }
    }

    /// The next index the copy needs.
    pub fn next(&self) -> Index {
        Index(self.held.0 + 1)
    }

    /// Steps `rows`, the stored bytes of committed entries from `next()` on, into `m`. Rows at or below what the member
    /// holds are skipped when they match it, so a page that overlaps the last one is harmless; one that differs halts.
    /// The host persists `Fed::ready` whatever `halt` says: a refusal at row k leaves the rows before it committed.
    pub fn feed<C: Command>(&mut self, m: &mut Member<C>, now: Timestamp, rows: &[Vec<u8>]) -> Fed<C>
    where
        Ev<C>: serde::de::DeserializeOwned,
    {
        let mut entries: Vec<LogEntry<Ev<C>>> = Vec::with_capacity(rows.len());
        let (mut at, mut prev) = (self.held, self.digest);
        let mut halt = None;
        for row in rows {
            let e: LogEntry<Ev<C>> = match codec::from_bytes(row) {
                Ok(e) => e,
                Err(x) => {
                    halt = Some(Halt::Undecodable { index: Some(at.next()), why: x.to_string() });
                    break;
                }
            };
            let d = digest(&e);
            if e.index <= at {
                let held = if e.index == self.held { Some(self.digest) } else { m.digest_of(e.index) };
                if held.is_some_and(|h| h != d) {
                    halt = Some(Halt::Chain { index: e.index, why: "differs from the row the copy holds".into() });
                    break;
                }
                continue;
            }
            if e.index != at.next() {
                halt = Some(Halt::Chain { index: e.index, why: format!("does not follow {}", at.0) });
                break;
            }
            if e.prev != prev {
                halt = Some(Halt::Chain { index: e.index, why: "prev is not the digest of the row before".into() });
                break;
            }
            (at, prev) = (e.index, d);
            entries.push(e);
        }
        let Some(top) = entries.last().map(|e| e.index) else { return Fed { ready: m.idle(now), halt } };
        let term = entries.iter().map(|e| e.term).max().unwrap_or(Term(0)).max(m.term());
        self.round += 1;
        let req = AppendReq {
            term,
            leader: IMPORTER,
            boot: 0,
            round: self.round,
            sent_at: now,
            prev_index: self.held,
            prev_term: Term(0),
            prev_digest: self.digest,
            entries,
            commit: top,
            transfer: None,
            lead_seq: 0,
        };
        let ready = m.step(now, IMPORTER, Message::Append(req));
        if let Some(r) = refusal(&ready) {
            halt = Some(Halt::Refused(r));
        }
        // What the member committed, which is what it stored of these rows.
        let held = m.state().commit;
        if halt.is_none() && held < top {
            halt = Some(Halt::Chain { index: held.next(), why: "the member did not store the row".into() });
        }
        self.held = held;
        self.digest = m.digest_of(held).unwrap_or(self.digest);
        Fed { ready, halt }
    }

    /// The `InstallSnapshot` for an archived range: the host copied the objects `source` names. The member asks for
    /// it in `Out::install`, and the importer goes on from `through` once `Member::install` took the loaded fold.
    pub fn install<C: Command>(&mut self, m: &mut Member<C>, now: Timestamp, source: InstallSource) -> Ready<C, ()> {
        self.round += 1;
        let (through, d) = (source.through, source.through_digest);
        let term = source.through_term.max(m.term());
        let req = InstallSnapshot { term, boot: 0, round: self.round, sent_at: now, lead_seq: 0, source };
        let r = m.step(now, IMPORTER, Message::Install(req));
        (self.held, self.digest) = (through, d);
        r
    }
}

/// The record `chain`'s group was bootstrapped from, rebuilt from `row`, its entry 1's stored bytes: the record whose
/// `bootstrap` gives entry 1's digest. Several names share a shape (`Tri` and `Na3`), so each is tried and only an
/// exact match is kept; `None` for a row that is no bootstrap `Config` or that no record rebuilds.
pub fn record_of<E: serde::de::DeserializeOwned + crate::fold::Record>(
    chain: ChainId,
    row: &[u8],
) -> Option<PlacementRecord> {
    let e1: LogEntry<E> = codec::from_bytes(row).ok()?;
    let EntryBody::Config(GroupConfig::Normal(p)) = &e1.body else { return None };
    if e1.index != Index(1) {
        return None;
    }
    let want = digest(&e1);
    const NAMES: [PlacementName; 6] = [
        PlacementName::Tri,
        PlacementName::Na3,
        PlacementName::Us2w,
        PlacementName::Eu2w,
        PlacementName::Apac2w,
        PlacementName::Solo,
    ];
    NAMES.into_iter().find_map(|name| {
        let rec = PlacementRecord {
            name,
            sites: p.voters().iter().map(|m| m.site).collect(),
            home: p.home(),
            generation: p.generation(),
            moving_from: None,
            at: e1.at,
        };
        let b = bootstrap::<E>(chain, &rec).ok()?;
        (digest(&b) == want).then_some(rec)
    })
}

impl Default for Importer {
    fn default() -> Self {
        Importer::new()
    }
}

/// The member's `verify` refusal in its answer to the importer, if any. The answer is still in the `Ready`: only
/// `persist` releases it, and the host drops it there.
fn refusal<C: Command>(r: &Ready<C, ()>) -> Option<Refusal> {
    r.out().messages.iter().find_map(|e| match &e.msg {
        Message::AppendRes(res) if e.to == IMPORTER => res.refused.clone(),
        _ => None,
    })
}
