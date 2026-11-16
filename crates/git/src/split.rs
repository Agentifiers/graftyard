//! A repository's metadata child group (`design/DESIGN.md`, Shape, `CHILD_GROUP_AT`; `decisions/DECISIONS.md`, "A split
//! moves only the text `decide` never reads"): the pure half. The parent leader's alarm (`workers/site/src/split.rs`)
//! runs the step `plan` names.
//!
//! - **Start.** At `SPLIT_AT` of member SQLite the parent records `MetaSplit { child: RepoMeta(repo) }`.
//! - **Seed.** At its newest committed metadata snapshot p, the parent records `MetaSeed { at: p, key }`, creates the
//!   child on its own placement record, and records the same `MetaSeed` there. Each child member restores that
//!   snapshot into its metadata tier, which no entry writes after, and serves reads of it once restored.
//! - **Commit point.** Once the child leader has restored it, the parent records `MetaDropped { seed: p }`. Then the
//!   leader records `MetaSlim` entries until one pass is done, each slimming the next `SLIM_ROWS` rows, at most about
//!   `SLIM_BYTES`, of its metadata tier (`slim`): a row last written at or before p loses the text no `decide` reads, which the child's snapshot
//!   holds, replaced by `SLIMMED`, and `provenance` and `path_set` rows, which no read serves, go. A row written after
//!   p keeps its text, so the child never needs a later entry and its log holds only its bootstrap, a `Noop` a term,
//!   and `MetaSeed`. Every table and column `decide` reads stays, so every command still decides and commits on the
//!   parent. A read fills a slimmed field from the child's row (`Filled`); a row the parent deleted is never filled.

use graftyard_fold::{Read, Rows, Store, Val};
use graftyard_raft::{Format, TierSet};
use graftyard_types::{ChainId, Index, Tier, Timestamp};
use serde::{Deserialize, Serialize};

use crate::key::R2Key;
use crate::task::TaskMeta;
use crate::tiers::{CommentRow, IssueRow, ReleaseRow, ReviewRow, get_rec, put_rec};

/// `RepoEvent::Split`, tag 43, which only a parent or child leader of this release records itself.
pub const REPO_FORMAT_SPLIT: Format = Format { family: 0x20, version: 20 };
/// Member SQLite no member of a repository's groups passes (`design/DESIGN.md`, `CHILD_GROUP_AT`).
pub const CHILD_GROUP_AT: u64 = 8 << 30;
/// Where the split starts, in `ctx.storage.sql.databaseSize` [guess].
pub const SPLIT_AT: u64 = 7 << 30;
/// Metadata rows one `MetaSlim` entry looks at [guess]: about 1 MB of rows read and rewritten, as an archive step's
/// batch. A metadata snapshot running on the writer copies each row it slims once into `snap_undo`.
pub const SLIM_ROWS: usize = 512;
/// Row bytes (key and columns) one `MetaSlim` entry looks at, past which it stops after the row that crossed it. At
/// 512 rows of about 1.8 KB, about 0.9 MB, followers' `/raft` calls took p90 1.8 s against CALL_DEADLINE's 1 s, and
/// paced entries still tripped STEP_DOWN 11 times in 9 minutes; at about 290 KB they took 0.4 to 0.55 s
/// [measured, `tests/am1/results/b030-split.md`, smokes 1 and 2]. 128 KiB keeps one entry's apply near 0.3 s.
pub const SLIM_BYTES: usize = 128 << 10;
/// The text of a slimmed row: a read takes it from the child's row. A body that is this string anyway reads the same
/// from the child.
pub const SLIMMED: &str = "\u{0}";
/// The metadata tables `slim` walks, in order: the first five keep their rows without their text, the last two, which
/// no read serves (`graftyard_api::hosted::UNSERVED`), lose their rows.
pub const SLIM_TABLES: [&str; 7] =
    ["issue", "comment", "review", "release", "agent_task_meta", "provenance", "path_set"];

/// Wire tags: postcard stores the variants by position (`crates/git/tests/wire_tags.golden`). Add only at the end.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SplitEntry {
    /// Core, on the parent: the split started.
    MetaSplit { child: ChainId },
    /// Core: the child starts from the parent's metadata snapshot `key` at parent index `at`.
    MetaSeed { at: Index, key: R2Key },
    /// Core and Metadata, on the parent: the commit point. From it the parent slims the rows of its metadata tier last
    /// written at or before `seed`, `MetaSeed`'s `at`, whose text the child's snapshot holds.
    MetaDropped { seed: Index },
    /// Core and Metadata, on the parent after `MetaDropped` until the pass is done: the next `SLIM_ROWS` rows.
    MetaSlim,
}

fn key(k: &str) -> Vec<Val> {
    vec![Val::Text(k.into())]
}

/// Where a group stands in a split, from its core tier.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SplitState {
    /// `MetaSplit`'s child and time.
    pub child: Option<(ChainId, Timestamp)>,
    pub seed: Option<(Index, R2Key)>,
    pub dropped: bool,
}

impl SplitState {
    pub fn of(core: &(impl Read + ?Sized)) -> SplitState {
        SplitState {
            child: get_rec(core, "split", &key("split")),
            seed: get_rec(core, "split", &key("seed")),
            dropped: get_rec::<Timestamp>(core, "split", &key("dropped")).is_some(),
        }
    }
}

/// The core row `MetaSeed` writes, whose `written` is the `MetaSeed` entry's index, where a child member restamps
/// the restored tier (`workers/site/src/load.rs`).
pub fn seed_key() -> Vec<Val> {
    key("seed")
}

pub(crate) fn write_core(w: &mut dyn Rows, s: &SplitEntry, at: Timestamp) {
    match s {
        SplitEntry::MetaSplit { child } => put_rec(w, "split", key("split"), &(*child, at)),
        SplitEntry::MetaSeed { at: p, key: k } => put_rec(w, "split", key("seed"), &(*p, k.clone())),
        SplitEntry::MetaDropped { .. } => put_rec(w, "split", key("dropped"), &at),
        SplitEntry::MetaSlim => {}
    }
}

/// `slim`'s cursor in the parent's metadata tier, which `MetaDropped` writes: the seed index, the table and the last
/// key it looked at, and whether its one pass over `SLIM_TABLES` is done.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlimCursor {
    pub seed: Index,
    pub table: u8,
    pub last: Vec<Val>,
    pub done: bool,
}

/// The metadata rows of a split event: `MetaDropped` starts the slim pass, and each `MetaSlim` takes its next rows.
pub(crate) fn write_meta(w: &mut dyn Rows, s: &SplitEntry) {
    match s {
        SplitEntry::MetaDropped { seed } if get_rec::<SlimCursor>(w, "split", &key("slim")).is_none() => {
            put_rec(w, "split", key("slim"), &SlimCursor { seed: *seed, ..SlimCursor::default() })
        }
        SplitEntry::MetaSlim => slim(w),
        _ => {}
    }
}

/// The slim cursor, once `MetaDropped` applied to this metadata tier.
pub fn slim_cursor(meta: &(impl Read + ?Sized)) -> Option<SlimCursor> {
    get_rec(meta, "split", &key("slim"))
}

/// Whether the parent's metadata tier is due a snapshot because its slim pass ended after its newest one: the entry
/// that wrote the done cursor is newer than every committed metadata snapshot. A snapshot at or after it holds the
/// slimmed tier, which an `InstallSnapshot` then names in place of the whole tier from before `MetaDropped`: site c's
/// install on B0-30's rerun loaded that one, 3.25 GB, beside the member's own 5.7 GB
/// (`tests/am1/results/b030-split.md`, "Cause 1").
pub fn slim_snapshot_due(core: &(impl Read + ?Sized), meta: &(impl Store + ?Sized)) -> bool {
    if !slim_cursor(meta).is_some_and(|c| c.done) {
        return false;
    }
    let Some(ended) = Store::get(meta, "split", &key("slim")).map(|s| s.written) else { return false };
    crate::tiers::snapshots(core, Tier::Metadata).last().is_none_or(|(to, _, _)| *to < ended)
}

/// `verify`'s rule for a split event over the group's core tier: each step once, in order, on the right group, and
/// `child` is whether the group is the child (`RepoFold::child`), `repo` its repository.
/// It reads only the core tier as committed, so a leader's own record never meets a refusal it could not foresee.
pub(crate) fn check(
    core: &(impl Read + ?Sized),
    repo: Option<graftyard_types::RepoId>,
    child: bool,
    s: &SplitEntry,
) -> Result<(), Box<str>> {
    let st = SplitState::of(core);
    match s {
        SplitEntry::MetaSplit { child: c } => {
            if child || st.child.is_some() {
                return Err("a split starts once, on the parent".into());
            }
            if repo.map(ChainId::RepoMeta) != Some(*c) {
                return Err(format!("the child of {repo:?} is not {c}").into());
            }
            Ok(())
        }
        SplitEntry::MetaSeed { at, key: k } => {
            if st.seed.is_some() || (!child && st.child.is_none()) {
                return Err("a seed comes once, after the split".into());
            }
            match k {
                R2Key::Snapshot { chain, tier: Tier::Metadata, to, .. }
                    if Some(*chain) == repo.map(ChainId::Repo) && to == at => {}
                _ => return Err(format!("{k} is not this repository's metadata snapshot at {at:?}").into()),
            }
            if !child && !crate::tiers::snapshots(core, Tier::Metadata).iter().any(|(_, _, a)| a.key == *k) {
                return Err(format!("{k} is not a committed snapshot").into());
            }
            Ok(())
        }
        SplitEntry::MetaDropped { seed } => match &st.seed {
            Some((p, _)) if !child && !st.dropped && p == seed => Ok(()),
            _ => Err("the parent drops its text once, after the seed, at the seed".into()),
        },
        SplitEntry::MetaSlim if child || !st.dropped => Err("the parent slims only after it dropped".into()),
        SplitEntry::MetaSlim => Ok(()),
    }
}

/// The tiers a split event writes.
pub(crate) fn tiers(s: &SplitEntry) -> TierSet {
    match s {
        SplitEntry::MetaDropped { .. } | SplitEntry::MetaSlim => TierSet::CORE.union(TierSet::METADATA),
        _ => TierSet::CORE,
    }
}

/// The child as the parent leader last read it (`{"q": {"split": true}}` on the child).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChildView {
    /// `MetaSeed` committed on the child.
    pub seeded: bool,
    /// The child leader's metadata tier holds the seed's snapshot and serves reads.
    pub loaded: bool,
}

/// What the parent leader's alarm does next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SplitStep {
    Idle,
    /// Record `MetaSplit` on the parent.
    Split,
    /// No metadata snapshot is committed yet.
    WaitSnapshot,
    /// Record `MetaSeed` on the parent.
    Seed {
        at: Index,
        key: R2Key,
    },
    /// Create the child, if the lookup finds none, and record the parent's `MetaSeed` there.
    Child {
        at: Index,
        key: R2Key,
    },
    /// Record `MetaDropped` on the parent.
    Drop {
        seed: Index,
    },
    /// Record `MetaSlim` on the parent: the slim pass is not done.
    Slim,
}

/// The parent leader's next step, from its tiers, its member SQLite, and the child as last read (`None` before any
/// read answered).
pub fn plan(
    core: &(impl Read + ?Sized),
    meta: &(impl Read + ?Sized),
    db_bytes: u64,
    split_at: u64,
    child: Option<&ChildView>,
) -> SplitStep {
    let st = SplitState::of(core);
    if st.child.is_none() {
        return if db_bytes >= split_at { SplitStep::Split } else { SplitStep::Idle };
    }
    let Some((at, key)) = st.seed else {
        return match crate::tiers::snapshots(core, Tier::Metadata).pop() {
            Some((to, _, a)) => SplitStep::Seed { at: to, key: a.key },
            None => SplitStep::WaitSnapshot,
        };
    };
    if st.dropped {
        return if slim_cursor(meta).is_some_and(|c| !c.done) { SplitStep::Slim } else { SplitStep::Idle };
    }
    match child.filter(|c| c.seeded) {
        None => SplitStep::Child { at, key },
        Some(c) if c.loaded => SplitStep::Drop { seed: at },
        Some(_) => SplitStep::Idle,
    }
}

/// One `MetaSlim`: the next rows of `SLIM_TABLES` after the cursor, in one pass, up to `SLIM_ROWS` rows and up to the
/// row that reaches `SLIM_BYTES`. A row last written after the seed keeps its text: the child's snapshot lacks it, and
/// the parent keeps every text written since. Every member writes the same rows.
fn slim(w: &mut dyn Rows) {
    let Some(mut cur) = slim_cursor(w).filter(|c| !c.done) else { return };
    let (mut left, mut bytes) = (SLIM_ROWS, 0);
    while left > 0 && bytes < SLIM_BYTES && !cur.done {
        let table = SLIM_TABLES[cur.table as usize];
        // A scan starts at its key, so the cursor's own row comes back first after the first batch.
        let want = left + 1 + usize::from(!cur.last.is_empty());
        let batch: Vec<_> =
            w.scan(table, &cur.last, want).into_iter().filter(|(k, _)| cur.last.is_empty() || *k != cur.last).collect();
        let mut end = batch.len() <= left;
        for (k, cols) in batch.into_iter().take(left) {
            if bytes >= SLIM_BYTES {
                end = false;
                break;
            }
            bytes += k.iter().chain(&cols).map(val_bytes).sum::<usize>();
            if w.written(table, &k).is_some_and(|i| i <= cur.seed) {
                slim_row(w, table, &k);
            }
            cur.last = k;
            left -= 1;
        }
        if end {
            cur.last = Vec::new();
            cur.table += 1;
            cur.done = cur.table as usize == SLIM_TABLES.len();
        }
    }
    put_rec(w, "split", key("slim"), &cur);
}

fn val_bytes(v: &Val) -> usize {
    match v {
        Val::Null => 1,
        Val::Int(_) => 8,
        Val::Text(t) => t.len(),
        Val::Blob(b) => b.len(),
    }
}

fn slim_row(w: &mut dyn Rows, table: &str, k: &[Val]) {
    fn redo<T: Serialize + serde::de::DeserializeOwned>(
        w: &mut dyn Rows,
        table: &str,
        k: &[Val],
        f: impl FnOnce(&mut T),
    ) {
        if let Some(mut r) = get_rec::<T>(w, table, k) {
            f(&mut r);
            put_rec(w, table, k.to_vec(), &r);
        }
    }
    let gone = || Box::<str>::from(SLIMMED);
    match table {
        "issue" => redo::<IssueRow>(w, table, k, |r| r.body = gone()),
        "comment" => redo::<CommentRow>(w, table, k, |r| r.body = gone()),
        "review" => redo::<ReviewRow>(w, table, k, |r| r.body = gone()),
        "release" => redo::<ReleaseRow>(w, table, k, |r| r.rec.body = gone()),
        "agent_task_meta" => {
            redo::<TaskMeta>(w, table, k, |r| (r.prompt, r.summary) = (r.prompt.as_ref().map(|_| gone()), gone()))
        }
        _ => w.delete(table, k),
    }
}

fn dec<T: serde::de::DeserializeOwned>(c: &[Val]) -> Option<T> {
    match c.first() {
        Some(Val::Blob(b)) => postcard::from_bytes(b).ok(),
        _ => None,
    }
}

/// Whether a metadata row holds a text field the child holds instead.
pub fn slimmed(table: &str, cols: &[Val]) -> bool {
    let is = |s: &str| s == SLIMMED;
    match table {
        "issue" => dec::<IssueRow>(cols).is_some_and(|r| is(&r.body)),
        "comment" => dec::<CommentRow>(cols).is_some_and(|r| is(&r.body)),
        "review" => dec::<ReviewRow>(cols).is_some_and(|r| is(&r.body)),
        "release" => dec::<ReleaseRow>(cols).is_some_and(|r| is(&r.rec.body)),
        "agent_task_meta" => {
            dec::<TaskMeta>(cols).is_some_and(|r| is(&r.summary) || r.prompt.as_deref() == Some(SLIMMED))
        }
        _ => false,
    }
}

/// A parent row with each slimmed text field taken from the child's row of the same key; every other field, and every
/// field the parent wrote since, is the parent's. With no child row a slimmed field reads empty.
pub fn fill(table: &str, parent: &[Val], child: Option<&[Val]>) -> Vec<Val> {
    fn enc<T: Serialize>(t: &T) -> Vec<Val> {
        vec![Val::Blob(postcard::to_allocvec(t).expect("a fold row encodes").into())]
    }
    fn take(mine: &mut Box<str>, theirs: Option<Box<str>>) {
        if &**mine == SLIMMED {
            *mine = theirs.unwrap_or_default();
        }
    }
    let c = child.unwrap_or(&[]);
    match table {
        "issue" => dec::<IssueRow>(parent).map(|mut p| {
            take(&mut p.body, dec::<IssueRow>(c).map(|c| c.body));
            enc(&p)
        }),
        "comment" => dec::<CommentRow>(parent).map(|mut p| {
            take(&mut p.body, dec::<CommentRow>(c).map(|c| c.body));
            enc(&p)
        }),
        "review" => dec::<ReviewRow>(parent).map(|mut p| {
            take(&mut p.body, dec::<ReviewRow>(c).map(|c| c.body));
            enc(&p)
        }),
        "release" => dec::<ReleaseRow>(parent).map(|mut p| {
            take(&mut p.rec.body, dec::<ReleaseRow>(c).map(|c| c.rec.body));
            enc(&p)
        }),
        "agent_task_meta" => dec::<TaskMeta>(parent).map(|mut p| {
            let c = dec::<TaskMeta>(c);
            take(&mut p.summary, c.as_ref().map(|c| c.summary.clone()));
            if p.prompt.as_deref() == Some(SLIMMED) {
                p.prompt = c.and_then(|c| c.prompt);
            }
            enc(&p)
        }),
        _ => None,
    }
    .unwrap_or_else(|| parent.to_vec())
}

/// The child's row of a table and key as a host holds it: `Some(None)` for no such row, `None` for one not read yet.
pub type ChildRow<'a> = dyn Fn(&str, &[Val]) -> Option<Option<Vec<Val>>> + 'a;

/// A parent's metadata tier as a read sees it: each slimmed row with its text filled from the child's row of the same
/// key, which `child` answers, `Some(None)` for no such row and `None` for a row it does not hold yet. A miss is
/// counted, and its field reads empty; the host fetches the missed rows and builds the read again. Only rows the parent
/// holds are filled, so a row the parent deleted stays deleted.
pub struct Filled<'a> {
    pub meta: &'a dyn Read,
    pub child: &'a ChildRow<'a>,
    pub missed: &'a std::cell::RefCell<Vec<(String, Vec<Val>)>>,
}

impl Filled<'_> {
    fn row(&self, table: &str, key: &[Val], cols: Vec<Val>) -> Vec<Val> {
        if !slimmed(table, &cols) {
            return cols;
        }
        match (self.child)(table, key) {
            Some(c) => fill(table, &cols, c.as_deref()),
            None => {
                self.missed.borrow_mut().push((table.to_owned(), key.to_vec()));
                fill(table, &cols, None)
            }
        }
    }
}

impl Read for Filled<'_> {
    fn get(&self, table: &str, key: &[Val]) -> Option<Vec<Val>> {
        let cols = self.meta.get(table, key)?;
        Some(self.row(table, key, cols))
    }
    fn scan(&self, table: &str, from: &[Val], limit: usize) -> Vec<(Vec<Val>, Vec<Val>)> {
        self.meta
            .scan(table, from, limit)
            .into_iter()
            .map(|(k, c)| {
                let c = self.row(table, &k, c);
                (k, c)
            })
            .collect()
    }
}
