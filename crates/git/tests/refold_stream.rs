//! The fold audit's streamed refold on `repo:65536`'s shape, scaled down (`decisions/DECISIONS.md`, "`refold-stream`"):
//! a core tier of many refs and leases and a metadata tier of many checks, snapshotted, then a tail that moves refs,
//! renews leases, prunes expired leases and checks across many pages in one entry, promotes, and resolves review
//! threads, whose unresolved count the fold reads and asserts on. At every index of the tail the staged refold and a
//! restore into a store agree, with pages of a few rows and a cache of one page.

#[path = "stream_eq/mod.rs"]
mod stream_eq;

use graftyard_git::repo::{RepoEvent, RepoFold};
use graftyard_raft::{EntryBody, Fold, Format, LogEntry, PREV_ZERO, Term, digest};
use graftyard_types::{AccountId, ChainId, DbId, Index, RepoId, Tier, Timestamp};
use serde_json::{Value, json};

const DAY: u64 = 86_400_000;

fn oid(n: u64) -> Value {
    let mut b = [0u8; 20];
    b[12..].copy_from_slice(&n.to_be_bytes());
    json!(b.to_vec())
}

fn name(s: &str) -> Value {
    json!(s.as_bytes().to_vec())
}

fn ulid(n: u64) -> String {
    format!("{n:026}")
}

fn ev(v: Value) -> RepoEvent {
    serde_json::from_value(v.clone()).unwrap_or_else(|e| panic!("{e}: {v}"))
}

fn refs(push: u64, updates: Vec<(String, Option<u64>, Option<u64>)>) -> RepoEvent {
    let u: Vec<Value> = updates
        .into_iter()
        .map(|(n, e, x)| json!({ "name": name(&n), "expect": e.map(oid), "new": x.map(oid) }))
        .collect();
    ev(json!({ "Refs": { "push": ulid(push), "actor": 5, "updates": u, "pack": { "inline": "UEFDSw==" } } }))
}

fn lease(id: u64, expires: u64) -> RepoEvent {
    ev(json!({ "LeaseAcquire": { "lease": {
        "id": ulid(id), "holder": { "actor": { "User": 5 }, "app": null, "session": null },
        "scope": { "paths": [], "refs": [] }, "mode": "Shared", "intent": "edit", "granted": 1000, "expires": expires,
    }, "plan": null } }))
}

fn check(run: u64, sha: u64) -> RepoEvent {
    ev(json!({ "CheckRun": {
        "run": run, "suite": 401, "app": 1, "head_sha": oid(sha), "name": format!("ci-{run}"), "version": 1,
        "status": "InProgress", "conclusion": null, "started": 1500, "completed": null, "external_id": null,
        "details_url": null, "output": null,
    } }))
}

fn thread(id: u64) -> RepoEvent {
    ev(json!({ "Comment": { "Created": {
        "id": id, "author": 5, "body": "x", "reply_to": null,
        "on": { "Review": { "n": 2, "review": null, "at": { "commit": oid(1), "path": "a.rs", "line": 1, "start_line": null, "side": "Right" } } },
    } } }))
}

fn resolved(id: u64, resolved: bool) -> RepoEvent {
    ev(json!({ "Comment": { "Resolved": { "thread": id, "resolved": resolved, "actor": 5 } } }))
}

struct Chain {
    fold: RepoFold,
    log: Vec<LogEntry<RepoEvent>>,
}

impl Chain {
    fn entry(&mut self, at: u64, events: Vec<RepoEvent>) {
        let tiers = events.iter().fold(graftyard_raft::TierSet::NONE, |t, e| t.union(<RepoFold as Fold>::tiers(e)));
        let e = LogEntry {
            term: Term(2),
            index: Index(self.log.len() as u64 + 1),
            prev: self.log.last().map_or(PREV_ZERO, digest),
            format: Format { family: 0x20, version: 25 },
            tiers,
            at: Timestamp(at),
            requests: Default::default(),
            extend: None,
            body: EntryBody::Events(events),
        };
        self.fold.apply_entry(&e);
        self.log.push(e);
    }
}

fn snapshot(
    store: &mut dyn graftyard_fold::Store,
    schema: graftyard_fold::Schema,
) -> Vec<graftyard_fold::SnapshotFrame> {
    let v1 = Format { family: 0x20, version: 1 };
    let mut job = graftyard_fold::SnapshotJob::begin(
        store,
        schema,
        ChainId::Repo(RepoId(DbId(7))),
        graftyard_types::ReleaseId([0; 32]),
        v1,
    )
    .unwrap();
    let mut frames = Vec::new();
    while let Some(f) = job.next(store, 7) {
        frames.push(f);
    }
    graftyard_fold::end_snapshot(store, &schema);
    frames
}

#[test]
fn a_streamed_refold_of_a_busy_chain_equals_a_restore_at_every_index() {
    let corpus: Value = serde_json::from_str(include_str!("fold_corpus.json")).unwrap();
    let mut c = Chain { fold: RepoFold::new(RepoId(DbId(7)), AccountId(DbId(1))), log: Vec::new() };
    // The corpus's setup opens pull request 2, a rule, a lease and a check suite, with 30-day check retention.
    let setup: Vec<RepoEvent> = corpus["setup"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| {
            let mut s = serde_json::to_string(v).unwrap();
            for (k, b) in [
                ("\"@ref:refs/heads/main\"", name("refs/heads/main")),
                ("\"@ref:refs/heads/feature\"", name("refs/heads/feature")),
            ] {
                s = s.replace(k, &b.to_string());
            }
            for h in ["aa", "bb"] {
                s = s.replace(
                    &format!("\"@oid:{h}\""),
                    &json!(vec![u8::from_str_radix(h, 16).unwrap(); 20]).to_string(),
                );
            }
            ev(serde_json::from_str(&s).unwrap())
        })
        .collect();
    c.entry(1_000, setup);
    for i in 0..30u64 {
        let updates = (0..12).map(|j| (format!("refs/heads/b{i:03}-{j}"), None, Some(i * 100 + j))).collect();
        let leases = (0..6).map(|j| lease(1_000 + i * 10 + j, 5 * DAY + (i * 6 + j) * 60_000)).collect::<Vec<_>>();
        let checks = (0..5).map(|j| check(10_000 + i * 10 + j, i)).collect::<Vec<_>>();
        let mut evs = vec![refs(100 + i, updates)];
        evs.extend(leases);
        evs.extend(checks);
        evs.push(thread(5_000 + i));
        c.entry(2_000 + i * 1_000, evs);
    }
    let from = Index(c.log.len() as u64);
    let core_frames = snapshot(&mut *c.fold.core, graftyard_git::tiers::CORE);
    let meta_frames = snapshot(&mut *c.fold.meta, graftyard_git::tiers::METADATA);
    // The tail: refs moved and deleted, leases renewed, threads resolved and reopened, a promotion, then entries whose
    // time passes many lease expiries at once, and one past the checks' retention.
    let mut at = 40_000;
    for i in 0..12u64 {
        let moved = (0..4).map(|j| {
            let n = format!("refs/heads/b{:03}-{j}", i * 2);
            let old = i * 2 * 100 + j;
            (n, Some(old), if j == 3 { None } else { Some(old + 50_000) })
        });
        c.entry(at, vec![refs(500 + i, moved.collect())]);
        at += 1_000;
        c.entry(
            at,
            vec![
                ev(json!({ "LeaseRenew": { "id": ulid(1_000 + i * 10), "expires": 40 * DAY } })),
                resolved(5_000 + i, true),
            ],
        );
        at += 1_000;
        if i % 3 == 0 {
            c.entry(at, vec![resolved(5_000 + i, false)]);
            at += 1_000;
        }
    }
    c.entry(
        at,
        vec![ev(json!({ "Promoted": { "from": 1, "to": 17, "pack": { "stored": {
        "id": ulid(9), "checksum": vec![5u8; 32], "bytes": 1000, "sites": 7 } } } }))],
    );
    for t in [5 * DAY + 40 * 60_000, 5 * DAY + 150 * 60_000, 6 * DAY, 31 * DAY] {
        c.entry(t, vec![refs(900 + t / 60_000, vec![("refs/heads/tick".into(), None, Some(t))])]);
    }
    let tail = &c.log[from.0 as usize..];
    let t = c.fold.tiers();
    let chain = ChainId::Repo(RepoId(DbId(7)));
    for (schema, frames) in [(graftyard_git::tiers::CORE, &core_frames), (graftyard_git::tiers::METADATA, &meta_frames)]
    {
        for page_bytes in [300, graftyard_fold::PAGE_BYTES] {
            let most = stream_eq::equal_at_every_index(&t, schema, chain, from, frames, tail, page_bytes);
            println!("{:?} tier, {page_bytes} B pages: at most {most} pages read by one entry", schema.tier);
        }
    }
    // The live members' digests are what the refold reaches.
    let core = graftyard_fold::fold_digest(&*c.fold.core, Tier::Core);
    let meta = graftyard_fold::fold_digest(&*c.fold.meta, Tier::Metadata);
    let end = |schema, frames: &Vec<_>| {
        graftyard_fold::refold_segment(&t, schema, (chain, from), frames.clone(), tail.iter().cloned()).unwrap()
    };
    assert_eq!(end(graftyard_git::tiers::CORE, &core_frames), core);
    assert_eq!(end(graftyard_git::tiers::METADATA, &meta_frames), meta);
    let checks = c.fold.meta.scan("check_run", &[], usize::MAX).len();
    assert_eq!(checks, 0, "the last entry pruned every check past its retention");
    let pruned = c.fold.core.scan("lease", &[], usize::MAX).len();
    assert!(pruned < 150, "the tail's entries pruned leases across pages: {pruned} of 181 left");
}
