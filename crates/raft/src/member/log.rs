//! The member's log, apply, and `verify`.

use graftyard_types::IdBlock;

use crate::encode::digest;
use crate::fold::{Command, Fold, Scratch};
use crate::types::{
    Digest, EntryBody, Format, GroupConfig, Index, LogEntry, Next, PREV_ZERO, RAFT_FORMAT, ReleaseTag, Term,
};

use super::{Member, Refusal, Write};

/// What a follower checks entry k+1 against: its fold at `applied`, the stored entries above it, and the tail it
/// must chain to.
pub struct FoldView<'a, F: Fold> {
    pub fold: &'a F,
    /// The latest committed `EntryBody::IdBlock`, which the next one must follow.
    pub id_block: Option<IdBlock>,
    /// Stored entries above `applied` and below the entry, in order. Usually none, because the append for k+1
    /// carries commit index k and the follower applies k first.
    pub unapplied: &'a [LogEntry<F::Event>],
    /// The index the entry must have, and the digest of the entry below it.
    pub index: Index,
    pub prev: Digest,
    pub boot: Digest,
    pub reads: &'a [Format],
    pub release: ReleaseTag,
}

/// Followers run this before they store entry k+1. The entry must chain to `prev`, use a format this release reads,
/// keep term 1 for the bootstrap entry alone, map each request to events it holds, and pass each event's check in
/// order on a scratch view over the fold, which is then dropped. An `IdBlock` carries no requests and starts at or
/// after the end of the group's last block, stored entries above `applied` included.
pub fn verify<F: Fold>(e: &LogEntry<F::Event>, view: &FoldView<F>) -> Result<(), Refusal> {
    let refuse = |reason: String| Err(Refusal { release: view.release, index: e.index, reason: reason.into() });
    if e.index != view.index {
        return refuse(format!("index {:?} where {:?} was expected", e.index, view.index));
    }
    if e.prev != view.prev {
        return refuse(format!("prev {:?} does not chain to {:?}", e.prev, view.prev));
    }
    if e.format != RAFT_FORMAT && !view.reads.contains(&e.format) {
        return refuse(format!("format {:?} is not read by this release", e.format));
    }
    if (e.index == Index(1)) != (e.term == Term(1)) {
        return refuse("term 1 holds only the bootstrap entry".into());
    }
    if e.index == Index(1) && digest(e) != view.boot {
        return refuse("bootstrap entry differs from the one built from the record".into());
    }
    if let EntryBody::IdBlock(b) = &e.body {
        let last = view.unapplied.iter().rev().find_map(|u| match &u.body {
            EntryBody::IdBlock(x) => Some(*x),
            _ => None,
        });
        if let Some(prev) = last.or(view.id_block)
            && !prev.followed_by(*b)
        {
            return refuse(format!("id block {b:?} does not follow {prev:?}"));
        }
    }
    let EntryBody::Events(events) = &e.body else {
        return if e.requests.is_empty() { Ok(()) } else { refuse("requests on an entry with no events".into()) };
    };
    let mut end = 0;
    for (id, r) in &e.requests {
        if r.start < end || r.end < r.start || r.end as usize > events.len() {
            return refuse(format!("request {id:?} maps to events {r:?} of {}", events.len()));
        }
        end = r.end;
    }
    let mut s = view.fold.scratch_at(e.at, e.tiers);
    for u in view.unapplied {
        match &u.body {
            EntryBody::Events(evs) => {
                for ev in evs {
                    s.apply(ev);
                }
            }
            EntryBody::IdBlock(b) => s.apply_id_block(*b),
            EntryBody::Config(_) | EntryBody::Noop => {}
        }
    }
    for (i, ev) in events.iter().enumerate() {
        if let Err(why) = s.check(ev) {
            return refuse(format!("event {i}: {why}"));
        }
        s.apply(ev);
    }
    Ok(())
}

impl<C: Command> Member<C> {
    pub(super) fn last(&self) -> Index {
        Index(self.base.index.0 + self.log.len() as u64)
    }

    pub(super) fn last_term(&self) -> Term {
        self.log.last().map_or(self.base.term, |e| e.term)
    }

    /// Position of index `i` in the in-memory window, which holds the entries above `base`.
    fn pos(&self, i: Index) -> Option<usize> {
        (i > self.base.index && i <= self.last()).then(|| (i.0 - self.base.index.0 - 1) as usize)
    }

    /// `None` below `base`, whose entries are on storage only.
    pub(super) fn term_at(&self, i: Index) -> Option<Term> {
        if i == self.base.index {
            return Some(self.base.term);
        }
        self.pos(i).map(|p| self.log[p].term)
    }

    pub(super) fn digest_at(&self, i: Index) -> Option<Digest> {
        if i == self.base.index {
            return Some(self.base.digest);
        }
        self.pos(i).map(|p| self.digests[p])
    }

    pub(super) fn entry(&self, i: Index) -> &LogEntry<crate::Ev<C>> {
        &self.log[self.pos(i).expect("entry in the window")]
    }

    /// Index 1 holds the bootstrap entry: in the window, or committed below `base`, where `verify` let in only that
    /// entry.
    pub(super) fn holds_boot(&self) -> bool {
        self.base.index >= Index(1) || self.digest_at(Index(1)) == Some(self.boot_digest)
    }

    pub(super) fn push_entry(&mut self, e: LogEntry<crate::Ev<C>>) {
        debug_assert_eq!(e.index, self.last().next());
        if matches!(e.body, EntryBody::Config(_)) {
            self.configs.push(e.index);
        }
        let (d, len) = crate::encode::digest_len(&e);
        self.digests.push(d);
        self.sizes.push(len);
        self.log_bytes += len;
        self.writes.push(Write::Append(e.clone()));
        if self.role == super::Role::Leader {
            self.own_appended = Some(e.index);
        }
        self.log.push(e);
    }

    /// Deletes every entry at or above `i`. A committed entry is never deleted: that would be a second value at a
    /// committed index.
    pub(super) fn truncate_from(&mut self, i: Index) {
        assert!(
            i > self.state.commit,
            "{:?}: truncate at {i:?} at or below commit {:?}",
            self.cfg.me,
            self.state.commit
        );
        if i > self.last() {
            return;
        }
        self.writes.push(Write::Truncate(i));
        self.epoch += 1;
        self.own_durable = self.own_durable.min(Index(i.0 - 1));
        self.own_appended = self.own_appended.filter(|a| *a < i);
        let keep = self.pos(i).expect("above commit, so in the window");
        self.log.truncate(keep);
        self.digests.truncate(keep);
        self.log_bytes -= self.sizes.drain(keep..).sum::<usize>();
        self.configs.retain(|c| *c < i);
    }

    /// The latest config in the log, committed or not, which counts votes and acks; the bootstrap config if the log
    /// is empty. A config below `base` is committed, so with none in the window the latest is the fold's.
    pub(super) fn log_config(&self) -> &GroupConfig {
        let e = match self.configs.last() {
            Some(i) => self.entry(*i),
            None => match self.fold.config() {
                Some((_, c)) => return c,
                None => &self.boot_entry,
            },
        };
        match &e.body {
            EntryBody::Config(c) => c,
            _ => unreachable!("configs lists only Config entries"),
        }
    }

    /// Raises `commit` and applies through it.
    pub(super) fn set_commit(&mut self, c: Index) {
        let c = c.min(self.last());
        if c <= self.state.commit {
            return;
        }
        self.state.commit = c;
        while self.fold.applied() < c {
            let next = self.fold.applied().next();
            let e = &self.log[self.pos(next).expect("unapplied entries are in the window")];
            self.fold.apply(e);
            // The marker clears when an entry carrying `extend` commits, or one stamped after it: the group committed
            // past it, and that entry's leader judged the time up to its `at`.
            if let Some(m) = self.state.stalled_since
                && (e.extend.is_some() || e.at > m)
            {
                self.state.stalled_since = None;
            }
        }
        self.state.applied = self.fold.applied();
        self.writes.push(Write::Apply(c));
        self.dirty = true;
        self.compact();
    }

    /// Drops applied entries from memory once more than `2 x window` have applied above `base`, keeping `window`, or
    /// once the applied ones pass `window_bytes`, keeping the newest that fit (and at least the last applied one).
    /// Never one applied in the current input: the host reads those from `log()` for its `Write::Apply`. They stay on
    /// the host's storage, and a follower that needs them gets them through `Out::catchup`.
    pub(super) fn compact(&mut self) {
        let applied = self.fold.applied();
        let held = applied.0.saturating_sub(self.base.index.0) as usize;
        let mut to = self.base.index;
        if applied.0 > self.base.index.0 + 2 * self.window {
            to = Index(applied.0 - self.window);
        }
        // The applied entries' bytes, newest first: everything below the newest run that fits goes.
        let mut kept = 0usize;
        for k in (0..held).rev() {
            kept += self.sizes[k];
            if kept > self.window_bytes && k + 1 < held {
                to = to.max(Index(self.base.index.0 + k as u64 + 1));
                break;
            }
        }
        let to = to.min(self.input_applied);
        if to <= self.base.index {
            return;
        }
        let n = (to.0 - self.base.index.0) as usize;
        let e = &self.log[n - 1];
        self.base = super::LogBase { index: to, term: e.term, digest: self.digests[n - 1] };
        self.log.drain(..n);
        self.digests.drain(..n);
        self.log_bytes -= self.sizes.drain(..n).sum::<usize>();
        self.configs.retain(|c| *c > to);
    }

    pub(super) fn view_for(&self, index: Index) -> FoldView<'_, C::Fold> {
        let base = self.base.index.0 as usize;
        let applied = self.fold.applied().0 as usize - base;
        let below = ((index.0 as usize).saturating_sub(1) - base).max(applied);
        FoldView {
            fold: &self.fold.fold,
            id_block: self.fold.id_block(),
            unapplied: &self.log[applied..below.min(self.log.len())],
            index,
            prev: self.digest_at(Index(index.0 - 1)).unwrap_or(PREV_ZERO),
            boot: self.boot_digest,
            reads: &self.cfg.reads,
            release: self.cfg.release,
        }
    }
}
