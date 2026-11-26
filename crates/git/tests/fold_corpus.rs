//! Every `RepoEvent` kind, committed at every format it can be committed at, folds to the core and metadata digests
//! the release that wrote it gave (F88). A committed entry folds the same on every release: a member that applied it
//! live and a fold audit's refold on a later release must agree, and roll-up's in-place `promoted_log` write broke
//! that for every pre-roll-up promotion. `fold_corpus.json` holds one fixture per kind, applied after a setup entry;
//! `fold_corpus.golden` holds `<kind> <format version> <core digest> <metadata digest>`. Its rows for versions up to 24
//! were pinned on main `0d212879` (the release before roll-up), and its 32.25 rows on `e2bddc92` (roll-up's merge).
//!
//! A digest that moves is a stored-format change. Changing a recorded digest is a deliberate edit that cites the
//! format bump and the dual-read release `design/SHARED.md`, "Schema change is a refold, never a migration", requires.
//! A new kind adds a fixture and its rows; a new format adds its rows and raises `NEWEST`. Print the current rows with
//! `GRAFTYARD_PIN=1 cargo test -p graftyard-git --test fold_corpus -- --nocapture`.

use graftyard_git::repo::{RepoEvent, RepoFold};
use graftyard_raft::{EntryBody, Fold, Format, LogEntry, PREV_ZERO, Term, digest};
use graftyard_types::{AccountId, DbId, Index, RepoId, Tier, Timestamp};
use serde_json::Value;

/// The newest repository format: version 35, F137's REST edits (`REPO_FORMAT_ISSUE_EDIT`).
const NEWEST: u16 = 35;

/// The first format version a kind can be committed at, by wire tag: the version that introduced it
/// (`crate::repo::RepoEvent`'s comments). Tags 0 to 26, Milestones 1 and 2, from version 1.
fn first_version(tag: usize) -> u16 {
    match tag {
        0..=26 => 1,
        27..=30 => 8,
        31 | 32 => 12,
        33 | 34 => 13,
        35..=38 => 14,
        39 => 15,
        40 | 41 => 16,
        42 => 19,
        43 => 20,
        44 | 45 => 23,
        46 => 24,
        47 => 25,
        48 => 30,
        49 => 33,
        50 => 34,
        _ => panic!("RepoEvent tag {tag}: name the format version that introduced it in first_version"),
    }
}

/// `"@ref:refs/x"` is a ref name's bytes, `"@oid:ab"` a 20-byte object id of `0xab`, `"@d:ab"` a 32-byte digest.
fn expand(v: Value) -> Value {
    match v {
        Value::String(s) => {
            let bytes = |b: Vec<u8>| Value::Array(b.into_iter().map(Value::from).collect());
            let fill = |h: &str, n: usize| bytes(vec![u8::from_str_radix(h, 16).expect("a hex byte"); n]);
            if let Some(r) = s.strip_prefix("@ref:") {
                bytes(r.as_bytes().to_vec())
            } else if let Some(h) = s.strip_prefix("@oid:") {
                fill(h, 20)
            } else if let Some(h) = s.strip_prefix("@d:") {
                fill(h, 32)
            } else {
                Value::String(s)
            }
        }
        Value::Array(a) => Value::Array(a.into_iter().map(expand).collect()),
        Value::Object(o) => Value::Object(o.into_iter().map(|(k, v)| (k, expand(v))).collect()),
        v => v,
    }
}

fn entry(index: u64, prev: graftyard_types::Digest, version: u16, events: Vec<RepoEvent>) -> LogEntry<RepoEvent> {
    let tiers = events.iter().fold(graftyard_raft::TierSet::NONE, |t, e| t.union(<RepoFold as Fold>::tiers(e)));
    LogEntry {
        term: Term(2),
        index: Index(index),
        prev,
        format: Format { family: 0x20, version },
        tiers,
        at: Timestamp(1_000 * index),
        requests: Default::default(),
        extend: None,
        body: EntryBody::Events(events),
    }
}

/// One tier's snapshot at `applied` as the snapshot writer writes it.
fn snapshot(
    store: &mut dyn graftyard_fold::Store,
    schema: graftyard_fold::Schema,
) -> Vec<graftyard_fold::SnapshotFrame> {
    let v1 = Format { family: 0x20, version: 1 };
    let chain = graftyard_types::ChainId::Repo(RepoId(DbId(7)));
    let mut job = graftyard_fold::SnapshotJob::begin(store, schema, chain, graftyard_types::ReleaseId([0; 32]), v1)
        .expect("no snapshot runs");
    let mut frames = Vec::new();
    while let Some(f) = job.next(store, 2) {
        frames.push(f);
    }
    graftyard_fold::end_snapshot(store, &schema);
    frames
}

/// The current tree's rows, and the kinds whose fixture this release does not decode. With `streamed`, each fixture
/// entry is folded as the fold audit refolds it (`decisions/DECISIONS.md`, "`refold-stream`"): each tier's snapshot
/// after the setup entry staged as pages of a few rows, and the entry applied through a `SnapView`.
fn rows(streamed: bool) -> (Vec<String>, Vec<String>) {
    let corpus: Value = serde_json::from_str(include_str!("fold_corpus.json")).expect("the corpus is JSON");
    let decode = |v: &Value| serde_json::from_value::<RepoEvent>(expand(v.clone()));
    let setup: Vec<RepoEvent> =
        corpus["setup"].as_array().unwrap().iter().map(|v| decode(v).expect("a setup event")).collect();
    let (mut out, mut skipped) = (Vec::new(), Vec::new());
    for (tag, v) in corpus["events"].as_array().unwrap().iter().enumerate() {
        let kind = v.as_object().and_then(|o| o.keys().next()).expect("one kind per fixture").clone();
        let Ok(ev) = decode(v) else {
            skipped.push(kind);
            continue;
        };
        for version in first_version(tag)..=NEWEST {
            let mut fold = RepoFold::new(RepoId(DbId(7)), AccountId(DbId(1)));
            let first = entry(1, PREV_ZERO, 1, setup.clone());
            fold.apply_entry(&first);
            let second = entry(2, digest(&first), version, vec![ev.clone()]);
            let (core, meta) = if streamed {
                let chain = graftyard_types::ChainId::Repo(RepoId(DbId(7)));
                let t = fold.tiers();
                let refold = |store: &mut dyn graftyard_fold::Store, schema: graftyard_fold::Schema| {
                    let frames = snapshot(store, schema);
                    graftyard_fold::refold_segment(&t, schema, (chain, Index(1)), frames, [second.clone()])
                        .expect("the fixture refolds")
                        .hex()
                };
                let core = refold(&mut *fold.core, graftyard_git::tiers::CORE);
                (core, refold(&mut *fold.meta, graftyard_git::tiers::METADATA))
            } else {
                fold.apply_entry(&second);
                let core = graftyard_fold::fold_digest(&*fold.core, Tier::Core).hex();
                (core, graftyard_fold::fold_digest(&*fold.meta, Tier::Metadata).hex())
            };
            out.push(format!("{kind} {version} {core} {meta}"));
        }
    }
    (out, skipped)
}

/// An entry's format is the highest of its commands' (`graftyard_raft`'s leader, `format.max`), so an event introduced
/// below a format already on main can commit inside an entry of that higher format, where a member that reads the
/// higher format but not the event's own passes the entry's format check and then meets a tag it cannot decode.
/// `first_version` therefore never falls as the tag rises (`decisions/DECISIONS.md`, "`format-order`"). AM3-WP2a's
/// `Genesis` first took 32.31 at tag 50, below `TaskResumed`'s 32.33 at tag 49; this check would have failed it.
#[test]
fn a_new_events_format_is_above_every_format_on_main() {
    let corpus: Value = serde_json::from_str(include_str!("fold_corpus.json")).expect("the corpus is JSON");
    let kinds = corpus["events"].as_array().unwrap().len();
    for tag in 1..kinds {
        let (before, now) = (first_version(tag - 1), first_version(tag));
        assert!(
            now >= before,
            "RepoEvent tag {tag} is introduced at 32.{now}, below tag {}'s 32.{before}: a new event's format must be \
             above every format on main, so take main's highest repository format plus one when it merges",
            tag - 1
        );
    }
    // A format may add no event kind (32.35, F137's REST edits, adds an `IssueOp` only), so the newest kind's format is
    // at most `NEWEST`, not always it.
    assert!(first_version(kinds - 1) <= NEWEST, "the newest tag's format is above NEWEST");
    let forks = graftyard_git::issue_ops::call::REPO_FORMAT_FORKS;
    assert_eq!((forks.family, forks.version), (0x20, first_version(50)), "Genesis (tag 50) rides REPO_FORMAT_FORKS");
}

#[test]
fn every_committed_event_folds_to_the_digests_its_release_gave() {
    check(false);
}

/// The fold audit's streamed refold gives the same digests as a live member for every fixture: the snapshot staged
/// as pages, the entry applied through `SnapView` (`decisions/DECISIONS.md`, "`refold-stream`").
#[test]
fn every_committed_event_refolds_through_staged_pages_to_the_same_digests() {
    check(true);
}

fn check(streamed: bool) {
    let (got, skipped) = rows(streamed);
    if std::env::var("GRAFTYARD_PIN").is_ok() && !streamed {
        for l in &got {
            println!("{l}");
        }
        return;
    }
    assert!(skipped.is_empty(), "fixtures this release does not decode: {skipped:?}");
    let want: Vec<&str> =
        include_str!("fold_corpus.golden").lines().filter(|l| !l.is_empty() && !l.starts_with('#')).collect();
    let key = |l: &str| l.split(' ').take(2).collect::<Vec<_>>().join(" ");
    let mut bad = Vec::new();
    for w in &want {
        match got.iter().find(|g| key(g) == key(w)) {
            Some(g) if g == w => {}
            Some(g) => {
                let k = key(w);
                bad.push(format!(
                    "{k}: the fold moved, from `{w}` to `{g}`. A committed entry must fold the same on every release; \
                     change what an existing event writes only for a new format, and change this row only with the \
                     dual-read release of design/SHARED.md, \"Schema change is a refold, never a migration\""
                ));
            }
            None => bad.push(format!("{}: no longer folded, but committed entries hold it", key(w))),
        }
    }
    for g in got.iter().filter(|g| !want.iter().any(|w| key(w) == key(g))) {
        bad.push(format!("new row `{g}`: add it to fold_corpus.golden"));
    }
    let tags = include_str!("wire_tags.golden").lines().filter(|l| l.starts_with("git::repo::RepoEvent ")).count();
    let corpus: Value = serde_json::from_str(include_str!("fold_corpus.json")).expect("the corpus is JSON");
    let kinds = corpus["events"].as_array().unwrap().len();
    assert_eq!(kinds, tags, "every RepoEvent kind in wire_tags.golden has a fixture in fold_corpus.json");
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}
