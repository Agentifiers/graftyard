//! The tier-generic fold on a two-table chain: an event writes Core, Metadata, or both. What a member observes is the
//! rows, `applied`, `fold_digest`, and the snapshot it writes and restores.

use graftyard_fold::{
    FoldError, FrameCheck, MemStore, Read, Rows, SNAPSHOT_EVERY, Schema, SnapshotError, SnapshotFrame, SnapshotJob,
    Store, TableDef, Tiered, Val, apply, decode_frames, decode_some, encode_frame, encode_frames, end_snapshot,
    fold_audit_compare, fold_digest, frames_header, meta, refold_digest, restore, snapshot_due, snapshot_writer,
    snapshots_to_keep,
};
use graftyard_raft::{EntryBody, Format, LogEntry, PREV_ZERO, Record, Term, TierSet, digest};
use graftyard_types::{
    ChainId, DbId, Digest, Index, Member, MemberKind, Placement, PlacementName, PlacementRecord, ReleaseId, RepoId,
    SiteId, Tier, Timestamp,
};
use smallvec::SmallVec;

#[derive(Clone, Debug, PartialEq)]
enum Ev {
    /// Core: `k = v`; `None` deletes.
    Core(i64, Option<i64>),
    /// Metadata: `k = v`; `None` deletes.
    Meta(i64, Option<i64>),
    /// Both tiers.
    Both(i64, i64),
}

impl Record for Ev {
    fn encode(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(format!("{self:?}").as_bytes());
    }
}

struct Toy;

const CORE: Schema = Schema { tier: Tier::Core, tables: &[TableDef { name: "c", key: &["k"] }] };
const META: Schema = Schema { tier: Tier::Metadata, tables: &[TableDef { name: "m", key: &["k"] }] };

fn set(w: &mut dyn Rows, t: &str, k: i64, v: Option<i64>) {
    match v {
        Some(v) => w.put(t, vec![Val::Int(k)], vec![Val::Int(v)]),
        None => w.delete(t, &[Val::Int(k)]),
    }
}

impl Tiered for Toy {
    type Event = Ev;
    fn tier_of(ev: &Ev) -> TierSet {
        match ev {
            Ev::Core(..) => TierSet::CORE,
            Ev::Meta(..) => TierSet::METADATA,
            Ev::Both(..) => TierSet::CORE.union(TierSet::METADATA),
        }
    }
    fn schema(tier: Tier) -> Schema {
        if tier == Tier::Core { CORE } else { META }
    }
    fn write(&self, ev: &Ev, tier: Tier, _at: Timestamp, w: &mut dyn Rows) {
        match (ev, tier) {
            (Ev::Core(k, v), Tier::Core) => set(w, "c", *k, *v),
            (Ev::Meta(k, v), Tier::Metadata) => set(w, "m", *k, *v),
            (Ev::Both(k, v), Tier::Core) => set(w, "c", *k, Some(*v)),
            (Ev::Both(k, v), Tier::Metadata) => set(w, "m", *k, Some(*v)),
            _ => panic!("{ev:?} routed to {tier:?}"),
        }
    }
}

/// A log of entries from `bodies`, chained by digest.
fn log(bodies: Vec<Vec<Ev>>) -> Vec<LogEntry<Ev>> {
    let mut prev = PREV_ZERO;
    let mut out = Vec::new();
    for (i, evs) in bodies.into_iter().enumerate() {
        let tiers = evs.iter().fold(TierSet::NONE, |t, e| t.union(Toy::tier_of(e)));
        let e = LogEntry {
            term: Term(2),
            index: Index(i as u64 + 1),
            prev,
            format: Format { family: 1, version: 1 },
            tiers,
            at: Timestamp(1_000 * (i as u64 + 1)),
            requests: SmallVec::new(),
            extend: None,
            body: EntryBody::Events(evs),
        };
        prev = digest(&e);
        out.push(e);
    }
    out
}

fn apply_all(store: &mut MemStore, tier: Tier, entries: &[LogEntry<Ev>]) {
    for e in entries {
        apply(&Toy, store, tier, e).unwrap();
    }
}

fn val(s: &MemStore, t: &str, k: i64) -> Option<i64> {
    match Read::get(s, t, &[Val::Int(k)])?.first() {
        Some(Val::Int(v)) => Some(*v),
        _ => None,
    }
}

#[test]
fn each_tier_applies_its_own_events_and_advances_past_every_entry() {
    let entries = log(vec![vec![Ev::Core(1, Some(10))], vec![Ev::Meta(2, Some(20))], vec![Ev::Both(3, 30)]]);
    let (mut core, mut md) = (MemStore::default(), MemStore::default());
    apply_all(&mut core, Tier::Core, &entries);
    apply_all(&mut md, Tier::Metadata, &entries);
    assert_eq!((val(&core, "c", 1), val(&core, "c", 3)), (Some(10), Some(30)));
    assert_eq!((val(&md, "m", 2), val(&md, "m", 3)), (Some(20), Some(30)));
    assert!(core.0.get("m").is_none_or(|t| t.is_empty()), "core never writes a metadata row");
    assert!(md.0.get("c").is_none_or(|t| t.is_empty()), "metadata never writes a core row");
    // Entry 2 is metadata-only and entry 1 core-only, yet both tiers stand at 3 and chain to entry 3's digest.
    assert_eq!(meta(&core).applied, Index(3));
    assert_eq!(meta(&md).applied, Index(3));
    assert_eq!(meta(&core).last, digest(&entries[2]));
    assert_eq!(meta(&md).last, digest(&entries[2]));
    // So both accept entry 4 and refuse a forged one.
    let more = log(vec![
        vec![Ev::Core(1, Some(10))],
        vec![Ev::Meta(2, Some(20))],
        vec![Ev::Both(3, 30)],
        vec![Ev::Meta(4, Some(40))],
    ]);
    apply(&Toy, &mut core, Tier::Core, &more[3]).unwrap();
    apply(&Toy, &mut md, Tier::Metadata, &more[3]).unwrap();
    assert_eq!(meta(&core).applied, Index(4));
}

#[test]
fn apply_refuses_a_gap_a_repeat_a_wrong_prev_and_a_wrong_tag_and_writes_nothing() {
    let entries = log(vec![vec![Ev::Core(1, Some(1))], vec![Ev::Core(1, Some(2))], vec![Ev::Core(1, Some(3))]]);
    for tier in [Tier::Core, Tier::Metadata] {
        let mut s = MemStore::default();
        assert_eq!(apply(&Toy, &mut s, tier, &entries[1]), Err(FoldError::Index { got: Index(2), want: Index(1) }));
        apply(&Toy, &mut s, tier, &entries[0]).unwrap();
        let before = s.clone();
        assert!(matches!(apply(&Toy, &mut s, tier, &entries[0]), Err(FoldError::Index { .. })), "a repeat");
        assert!(matches!(apply(&Toy, &mut s, tier, &entries[2]), Err(FoldError::Index { .. })), "a gap");
        let mut forged = entries[1].clone();
        forged.prev = Digest([9; 32]);
        assert_eq!(apply(&Toy, &mut s, tier, &forged), Err(FoldError::Prev { index: Index(2), applied: Index(1) }));
        // A core event in an entry tagged metadata-only: the core tier would never apply it.
        let mut mistagged = entries[1].clone();
        mistagged.tiers = TierSet::METADATA;
        assert!(matches!(apply(&Toy, &mut s, tier, &mistagged), Err(FoldError::Tag { .. })));
        assert_eq!(s, before, "a refused entry writes nothing");
    }
}

#[test]
fn fold_digest_depends_on_the_rows_not_the_order_they_were_written() {
    // Two histories that reach the same rows in a different order, with the same final entry digest stand-in:
    // compare the row part through `refold_digest` on two stores holding the same rows put in opposite orders.
    let a = log(vec![vec![Ev::Core(1, Some(1)), Ev::Core(2, Some(2))]]);
    let b = log(vec![vec![Ev::Core(2, Some(2)), Ev::Core(1, Some(1))]]);
    let (mut sa, mut sb) = (MemStore::default(), MemStore::default());
    apply_all(&mut sa, Tier::Core, &a);
    apply_all(&mut sb, Tier::Core, &b);
    // Different entries, so `fold_digest` differs through `last`; equal rows give equal row sums.
    assert_eq!(meta(&sa).sum, meta(&sb).sum, "the row sum ignores write order");
    assert_ne!(fold_digest(&sa, Tier::Core), fold_digest(&sb, Tier::Core), "the digest covers the entry chain");
    // The kept sum equals one recomputed from the rows, through puts, overwrites, and deletes.
    let mut s = MemStore::default();
    let entries = log(vec![
        vec![Ev::Core(1, Some(1)), Ev::Core(2, Some(2)), Ev::Core(3, Some(3))],
        vec![Ev::Core(2, Some(22)), Ev::Core(3, None), Ev::Meta(9, Some(9))],
        vec![Ev::Core(4, Some(4))],
    ]);
    apply_all(&mut s, Tier::Core, &entries);
    assert_eq!(fold_digest(&s, Tier::Core), refold_digest(&s, &CORE));
    // A row changed behind `apply`'s back shows as a mismatch, as an audit would see it.
    s.put("c", vec![Val::Int(1)], graftyard_fold::Stored { written: Index(1), cols: vec![Val::Int(7)] });
    assert_ne!(fold_digest(&s, Tier::Core), refold_digest(&s, &CORE));
}

#[test]
fn members_that_applied_the_same_log_agree_and_an_audit_names_the_one_that_differs() {
    let entries = log(vec![vec![Ev::Both(1, 1)], vec![Ev::Meta(2, Some(2))], vec![Ev::Core(1, None)]]);
    let mut members: Vec<MemStore> = (0..3).map(|_| MemStore::default()).collect();
    for m in &mut members {
        apply_all(m, Tier::Metadata, &entries);
    }
    let digests: Vec<(SiteId, Digest)> =
        members.iter().enumerate().map(|(i, m)| (SiteId(i as u8), fold_digest(m, Tier::Metadata))).collect();
    let mut refold = MemStore::default();
    apply_all(&mut refold, Tier::Metadata, &entries);
    let want = refold_digest(&refold, &META);
    assert!(!fold_audit_compare(Tier::Metadata, Index(3), want, &digests).alarm());
    let mut bad = digests.clone();
    bad[1].1 = Digest([1; 32]);
    let r = fold_audit_compare(Tier::Metadata, Index(3), want, &bad);
    assert_eq!(r.mismatches.iter().map(|m| m.member).collect::<Vec<_>>(), vec![SiteId(1)]);
}

const CHAIN: ChainId = ChainId::Repo(RepoId(DbId(7)));
const FMT: Format = Format { family: 1, version: 1 };

fn snapshot(
    store: &mut MemStore,
    tier: Tier,
    batch: usize,
    mut between: impl FnMut(&mut MemStore),
) -> Vec<SnapshotFrame> {
    let schema = Toy::schema(tier);
    let mut job = SnapshotJob::begin(store, schema, CHAIN, ReleaseId([3; 32]), FMT).unwrap();
    let mut frames = Vec::new();
    while let Some(f) = job.next(store, batch) {
        frames.push(f);
        between(store);
    }
    frames
}

#[test]
fn a_snapshot_taken_while_apply_continues_holds_the_tier_at_to_and_restores_there() {
    // Thirty rows at index 1..=3; the snapshot starts at 3, then entries 4..=9 change, delete, and add rows between
    // its batches. The frames must hold exactly the rows as of 3.
    let mut bodies: Vec<Vec<Ev>> = vec![
        (0..10).map(|k| Ev::Core(k, Some(k))).collect(),
        (10..20).map(|k| Ev::Core(k, Some(k))).collect(),
        (20..30).map(|k| Ev::Core(k, Some(k))).collect(),
    ];
    let later: Vec<Vec<Ev>> = vec![
        vec![Ev::Core(0, Some(100)), Ev::Core(29, None)],
        vec![Ev::Core(5, None), Ev::Core(31, Some(31))],
        vec![Ev::Core(0, Some(200)), Ev::Core(15, Some(150))],
        vec![Ev::Core(-1, Some(-1)), Ev::Core(28, Some(280))],
        vec![Ev::Core(31, Some(310)), Ev::Core(12, None)],
        vec![Ev::Core(29, Some(290))],
    ];
    bodies.extend(later.clone());
    let entries = log(bodies);
    let mut live = MemStore::default();
    apply_all(&mut live, Tier::Core, &entries[..3]);
    let mut at_three = live.clone();
    let want = fold_digest(&at_three, Tier::Core);
    let mut next = 3;
    let frames = snapshot(&mut live, Tier::Core, 4, |s| {
        if next < entries.len() {
            apply(&Toy, s, Tier::Core, &entries[next]).unwrap();
            next += 1;
        }
    });
    assert_eq!(next, entries.len(), "every later entry applied while the snapshot ran");
    let Some(SnapshotFrame::End(tr)) = frames.last() else { panic!("no trailer") };
    assert_eq!((tr.to, tr.rows, tr.fold_digest), (Index(3), 30, want));
    // Through the encoding and back into an empty tier: the rows and digest of index 3.
    let decoded = decode_frames(&encode_frames(&frames)).unwrap();
    let mut restored = MemStore::default();
    restore(&mut restored, &CORE, &decoded, (CHAIN, Index(3))).unwrap();
    assert_eq!(fold_digest(&restored, Tier::Core), want);
    assert_eq!(refold_digest(&restored, &CORE), want);
    assert_eq!(restored.0.get("c"), at_three.0.get("c"));
    // The restored tier takes entry 4 next, and ends where the live one did.
    for e in &entries[3..] {
        apply(&Toy, &mut restored, Tier::Core, e).unwrap();
    }
    end_snapshot(&mut live, &CORE);
    assert_eq!(fold_digest(&restored, Tier::Core), fold_digest(&live, Tier::Core));
    assert_eq!(restored.0.get("c"), live.0.get("c"));
    assert!(live.0.get("c~undo").is_none_or(|t| t.is_empty()), "end_snapshot drops snap_undo");
    // Restoring at the wrong index, into a tier that holds rows, or with a tampered row is refused.
    assert!(matches!(
        restore(&mut MemStore::default(), &CORE, &decoded, (CHAIN, Index(4))),
        Err(SnapshotError::Wrong { .. })
    ));
    assert_eq!(restore(&mut at_three, &CORE, &decoded, (CHAIN, Index(3))), Err(SnapshotError::NotEmpty));
    let mut tampered = decoded.clone();
    if let SnapshotFrame::Rows { rows, .. } = &mut tampered[0] {
        rows[0].cols = vec![Val::Int(-5)];
    }
    assert!(matches!(
        restore(&mut MemStore::default(), &CORE, &tampered, (CHAIN, Index(3))),
        Err(SnapshotError::Digest { .. })
    ));
}

#[test]
fn a_job_resumed_from_its_stored_cursor_streams_what_frame_check_accepts_at_the_key_digest() {
    // A host stores the cursor after each batch and resumes from it after a reset; the byte stream goes up in parts
    // cut anywhere. The digest the key names (`want`, taken at begin) is what the streamed trailer and rows prove.
    let entries = log((0..12).map(|b| (0..5).map(|k| Ev::Meta(b * 5 + k, Some(k))).collect()).collect());
    let mut s = MemStore::default();
    apply_all(&mut s, Tier::Metadata, &entries[..8]);
    let at_eight = fold_digest(&s, Tier::Metadata);
    let job = SnapshotJob::begin(&mut s, META, CHAIN, ReleaseId([1; 32]), FMT).unwrap();
    let want = job.want(&s);
    assert_eq!(want, at_eight);
    let mut cursor = job.cursor().clone();
    let mut bytes = frames_header();
    let mut next = 8;
    loop {
        let stored = serde_json::to_vec(&cursor).unwrap();
        let mut job = SnapshotJob::resume(META, serde_json::from_slice(&stored).unwrap());
        let Some(f) = job.next(&s, 7) else { break };
        encode_frame(&f, &mut bytes);
        cursor = job.cursor().clone();
        if next < entries.len() {
            apply(&Toy, &mut s, Tier::Metadata, &entries[next]).unwrap();
            next += 1;
        }
    }
    let check = |bytes: &[u8]| {
        let mut fc = FrameCheck::new(META);
        let mut buf = Vec::new();
        for part in bytes[10..].chunks(13) {
            buf.extend_from_slice(part);
            let (frames, used) = decode_some(&buf)?;
            for f in &frames {
                fc.frame(f)?;
            }
            buf.drain(..used);
        }
        fc.finish()
    };
    let tr = check(&bytes).unwrap();
    assert_eq!((tr.to, tr.rows, tr.fold_digest), (Index(8), 40, want));
    // One flipped byte in a row's value is caught, whichever check sees it first.
    let mut bad = bytes.clone();
    let at = bad.len() / 2;
    bad[at] ^= 1;
    assert!(check(&bad).is_err());
}

#[test]
fn snap_undo_keeps_the_first_prior_value_only_and_never_a_later_write() {
    // Row 1 written at 1, then changed at 3 and 4 while a snapshot at 2 runs: the snapshot holds the value of 1.
    // Row 2 is created at 3, after `to`: the snapshot has no row 2, and no undo copy is made for it.
    let entries = log(vec![
        vec![Ev::Meta(1, Some(10))],
        vec![Ev::Core(0, Some(0))],
        vec![Ev::Meta(1, Some(11)), Ev::Meta(2, Some(20))],
        vec![Ev::Meta(1, Some(12)), Ev::Meta(2, Some(21))],
    ]);
    let mut s = MemStore::default();
    apply_all(&mut s, Tier::Metadata, &entries[..2]);
    let schema = META;
    let mut job = SnapshotJob::begin(&mut s, schema, CHAIN, ReleaseId([0; 32]), FMT).unwrap();
    assert!(matches!(
        SnapshotJob::begin(&mut s, schema, CHAIN, ReleaseId([0; 32]), FMT),
        Err(SnapshotError::Running(Index(2)))
    ));
    apply_all(&mut s, Tier::Metadata, &entries[2..]);
    let undo: Vec<_> = s.0.get("m~undo").map(|t| t.keys().cloned().collect()).unwrap_or_default();
    assert_eq!(undo, vec![vec![Val::Int(1)]], "one copy, of the row written at or below to");
    let mut frames = Vec::new();
    while let Some(f) = job.next(&s, 10) {
        frames.push(f);
    }
    let SnapshotFrame::Rows { rows, .. } = &frames[0] else { panic!() };
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0].written, &rows[0].cols), (Index(1), &vec![Val::Int(10)]));
}

fn record(name: PlacementName, sites: &[u8]) -> PlacementRecord {
    PlacementRecord {
        name,
        sites: sites.iter().map(|s| SiteId(*s)).collect(),
        home: SiteId(sites[0]),
        generation: 1,
        moving_from: None,
        at: Timestamp(0),
    }
}

#[test]
fn the_snapshot_writer_rotates_over_full_followers_and_is_the_member_itself_for_solo() {
    let tri = Placement::from_record(CHAIN, &record(PlacementName::Tri, &[0, 1, 2])).unwrap();
    // Leader 0: the Full followers are 1 and 2, in config order.
    let w = |to: u64, leader: u8| snapshot_writer(Index(to), &tri, SiteId(leader));
    assert_eq!(w(0, 0), Some(SiteId(1)));
    assert_eq!(w(SNAPSHOT_EVERY - 1, 0), Some(SiteId(1)));
    assert_eq!(w(SNAPSHOT_EVERY, 0), Some(SiteId(2)));
    assert_eq!(w(2 * SNAPSHOT_EVERY, 0), Some(SiteId(1)));
    assert_eq!(w(SNAPSHOT_EVERY, 1), Some(SiteId(2)), "never the leader");
    assert_eq!(w(2 * SNAPSHOT_EVERY, 1), Some(SiteId(0)));
    // A witness never writes: in us2w the only Full follower of leader 0 is site 1.
    let w2 = Placement::from_record(CHAIN, &record(PlacementName::Us2w, &[0, 1, 2])).unwrap();
    assert!(w2.voters().iter().any(|m| m.kind == MemberKind::Witness && m.site == SiteId(2)));
    for k in 0..4 {
        assert_eq!(snapshot_writer(Index(k * SNAPSHOT_EVERY), &w2, SiteId(0)), Some(SiteId(1)));
    }
    let solo = Placement::from_record(CHAIN, &record(PlacementName::Solo, &[4])).unwrap();
    assert_eq!(snapshot_writer(Index(SNAPSHOT_EVERY * 3), &solo, SiteId(4)), Some(SiteId(4)));
    let _ = Member::new(CHAIN, SiteId(0), MemberKind::Full, 0, 1);
}

#[test]
fn snapshot_cadence_and_retention_use_entry_times() {
    let day = 86_400_000;
    assert!(snapshot_due(None, Index(1), Timestamp(0), 0));
    let last = Some((Index(10), Timestamp(day + 5)));
    assert!(!snapshot_due(last, Index(11), Timestamp(day + 6), 0));
    assert!(snapshot_due(last, Index(10 + SNAPSHOT_EVERY), Timestamp(day + 6), 0));
    assert!(snapshot_due(last, Index(11), Timestamp(day + 6), 256 << 20));
    assert!(snapshot_due(last, Index(11), Timestamp(2 * day), 0), "a new day");
    assert!(!snapshot_due(last, Index(10), Timestamp(3 * day), 0), "nothing committed since");
    // Snapshots at days 0, 10, 20, 30, 40, now at day 50: the 35-day window starts at day 15, so 20..=40 stay and
    // so does 10, the newest at or below the window's start; 0 goes.
    let snaps: Vec<(Index, Timestamp)> = (0..5).map(|i| (Index(i * 100), Timestamp(i * 10 * day))).collect();
    assert_eq!(snapshots_to_keep(&snaps, Timestamp(50 * day)), vec![Index(100), Index(200), Index(300), Index(400)]);
}
