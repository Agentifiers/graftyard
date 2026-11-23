//! A repository leader's per-entry CPU by stage, in local workerd (`tests/leaderprof/run.sh`): wasm32 under V8 with
//! real SQLite, the member's own `workers/site/src/tier.rs` (tier tables, `seal`) and `txn.rs` (`commit_tx` inside
//! `transactionSync`). A profile, not a gate: it prices what the hosted leader does per entry and per command on
//! B0-33's mix, so the hosted CPU (`tests/am1/results/b033-percmd.md`, `b033-quarter.md`) can be split by stage
//! (`tests/am1/results/b033-profile.md`).
//!
//! `/prof?cmds=N&entries=E&peers=P&grow=G` on object `L` applies G untimed entries (to grow the fold, tier rows and
//! log), then E timed entries of N commands each, timing every stage with the isolate's clock summed over all
//! entries. `only=K1,K2` keeps those kinds of the mix. The stages, each one piece of what `workers/site` does:
//! - `commit_decode`: each command's `/commit` body decoded (a push's as `RepoCmd`, an issue command's ~11 KB body,
//!   which carries the edge's ~10 KB render, as JSON).
//! - `fold_apply`, `seal`, `log_row`, `request_rows`, `sql_tx`: `SqlStore`'s `Write::Append` and `Write::Apply`.
//! - `peer_encode`: one AppendEntries envelope per peer (2 voters and 4 learners), as `Host::call` encodes it.
//! - `peer_send_js`: the body's Uint8Array, `RequestInit` and `Request` per peer; `peer_send_wall` the whole send to a
//!   sink object, which is wall time, not this isolate's CPU.
//! - `learner_call_empty` and `learner_call_append`: a learner's `/raft` call decoded and answered, without and with
//!   the held entry.
//! - `entries_page`: the `/entries` page the git cache shard reads, as `Host::entries_json` builds it.
//! - `reply_render`: each command's done reply, as a stub JSON (kept for comparison with earlier runs).
//! - The leader's done replies as `workers/site` builds them, each stage over the entry's commands: `waiter_clone`
//!   (the `Acked<RepoOut>` each waiter gets, `Host::release`), `render_parse` and `render_fill` (an issue command's
//!   `Render` decoded and `hosted::fill_authz`), `render_ok` (`Repo::out_json`: `hosted::render_ok` over the tiers
//!   after the entry, or a push's `PushReport::of`, by kind in `renderByKind`), `reply_build` (`outcome_json` and
//!   `ack_json`), `reply_send` (`Member::reply`'s `Response::from_json`), and `reply_reparse` (a test install's
//!   `receipt_fault`, which reads each answer back and parses it). Renders are the edge's first-send renders, with no
//!   owner `Authz`, filled from one shared fold the size of m3org's (`owner_authz`).
//! - `feed_ring`: `Repo::stepped`'s ring feed, which runs only on a member whose feed owner a poll read (never on
//!   B0-33, which runs no pollers).
//! - `consumer_clone`, `prsync_touched`, `queue_touched`: the leader's PrSync and merge-queue scans of the entry, as
//!   `workers/site/src/prsync.rs` and `merge_queue.rs` read it: two clones of the entry from the member, then
//!   `prsync::touched` and `queue_consumer::touched`.
//! - Probe reads, timed apart from the entry's work over PROBE_REPS calls and reported per call: `pulls_from`
//!   (`pull_by_head` for a pushed branch with no pull request that sorts among the open ones' heads),
//!   `pulls_from_after` (one that sorts after all of them), `lease_rows` (every lease row, as `LeaseView::live`
//!   reads them), and `scan_one` and `scan_256` (a one-row and a 256-row fold scan, one batch of
//!   `graftyard_fold::Read::prefix`).
//!
//! - `memo=0` reads SQLite on every fold read, as before `read-memo`; the default is the product's memo
//!   (`workers/site/src/tier.rs`, "Read memo"). `foldReads` answers each tier's (SQLite reads, memo answers).
//! - `/memo_scratch` and `/memo`: read-memo's checks (`tests/leaderprof/memo.sh`).
//!
//! The member DDL below is copied from `workers/site/src/host.rs` `SCHEMA`; run.sh fails if any line is not there
//! verbatim.
use graftyard_git::repo::{RepoCmd, RepoEvent, RepoFold, RepoOut};
use graftyard_raft::{
    AppendReq, EntryBody, Envelope, Fold, Format, LogEntry, Message, PREV_ZERO, Term, TierSet, codec,
};
use graftyard_types::{AccountId, DbId, Index, RepoId, RequestId, SiteId, Timestamp, Ulid};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use worker::*;

mod family {
    pub type Stmt = (&'static str, Vec<worker::SqlStorageValue>);
}

mod host {
    pub fn text(v: &worker::SqlStorageValue) -> worker::Result<&str> {
        match v {
            worker::SqlStorageValue::String(s) => Ok(s),
            other => Err(worker::Error::RustError(format!("expected text, got {other:?}"))),
        }
    }
}

mod ack;

#[path = "../../../../workers/site/src/tier.rs"]
#[allow(dead_code, unexpected_cfgs)]
mod tier;

#[path = "../../../../workers/site/src/txn.rs"]
#[allow(dead_code)]
mod txn;

const SCHEMA: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS raft_state (id INTEGER PRIMARY KEY CHECK (id = 1), row BLOB NOT NULL)",
    "CREATE TABLE IF NOT EXISTS log (idx INTEGER PRIMARY KEY, term INTEGER NOT NULL, digest BLOB NOT NULL, row BLOB NOT NULL)",
    "CREATE TABLE IF NOT EXISTS fold (id INTEGER PRIMARY KEY CHECK (id = 1), row BLOB NOT NULL)",
    "CREATE TABLE IF NOT EXISTS request (rid TEXT PRIMARY KEY, idx INTEGER NOT NULL, at INTEGER NOT NULL, events BLOB NOT NULL)",
    "CREATE INDEX IF NOT EXISTS request_at ON request (at)",
];
const CORPUS: &str = include_str!("../../../../crates/git/tests/fold_corpus.json");
/// The newest repository format (`crates/git/tests/fold_corpus.rs`, `NEWEST`).
const NEWEST: u16 = 25;
/// Agent-class inline pack bytes per entry (`graftyard_raft::admit::AGENT_INLINE_ENTRY`).
const AGENT_INLINE_ENTRY: usize = 128 << 10;
/// Learner `/raft` calls the leader answers per entry: about 50 a second over 3.7 to 4.2 entries a second at a quarter
/// load (`tests/am1/results/b033-quarter.md`).
const LEARNER_CALLS: usize = 12;
/// `/entries` pages the leader serves per entry: 7.5 to 9 a second over the same entries (same file).
const ENTRIES_CALLS: usize = 2;

const STAGES: &[(&str, &str)] = &[
    ("commit_decode", "command"),
    ("fold_apply", "entry"),
    ("seal", "entry"),
    ("log_row", "entry"),
    ("request_rows", "command"),
    ("sql_tx", "entry"),
    ("peer_encode", "peer"),
    ("peer_send_js", "peer"),
    ("peer_send_wall", "peer"),
    ("learner_call_empty", "call"),
    ("learner_call_append", "call"),
    ("entries_page", "call"),
    ("reply_render", "command"),
    ("render_parse", "issue command"),
    ("render_fill", "issue command"),
    ("render_ok", "command"),
    ("reply_build", "command"),
    ("reply_send", "command"),
    ("waiter_clone", "command"),
    ("reply_reparse", "command"),
    ("feed_ring", "entry"),
    ("consumer_clone", "entry"),
    ("prsync_touched", "entry"),
    ("queue_touched", "entry"),
    ("pulls_from", "call"),
    ("pulls_from_after", "call"),
    ("lease_rows", "call"),
    ("scan_one", "call"),
    ("scan_256", "call"),
];
/// Times each probe read (`pulls_from`, `pulls_from_after`, `lease_rows`, `scan_one`) runs per entry.
const PROBE_REPS: u64 = 10;

fn now() -> f64 {
    js_sys::Date::now()
}

/// `fold_corpus.rs`'s fixture shorthand: `"@ref:x"` a ref name's bytes, `"@oid:ab"` 20 bytes, `"@d:ab"` 32 bytes.
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

fn decode(v: Value) -> Result<RepoEvent> {
    serde_json::from_value::<RepoEvent>(expand(v)).map_err(|e| Error::RustError(e.to_string()))
}

/// 20 bytes unique to `k`, as an expanded oid.
fn oid(k: u64) -> Value {
    let mut b = [7u8; 20];
    b[..8].copy_from_slice(&k.to_be_bytes());
    Value::Array(b.iter().map(|x| Value::from(*x)).collect())
}

/// A 26-character ULID string for `k`.
fn ulid(k: u64) -> String {
    Ulid(u128::from(k) + 0x0193_0000_0000_0000_0000_0000_0000).to_string()
}

fn b64(bytes: &[u8]) -> String {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n =
            (u32::from(c[0]) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= c.len() {
                out.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Bytes that do not compress, as a pack's would not.
fn noise(n: usize, seed: u64) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}

/// One command of the mix: its events and its `/commit` body.
struct Cmd {
    events: Vec<RepoEvent>,
    body: Vec<u8>,
    push: bool,
    kind: &'static str,
    k: u64,
}

/// B0-33's committed mix, per N commands: agent pushes 25%, task edits 30%, comments 12%, labels 8%, lease renews 14%,
/// lease acquires 6%, draft PRs the rest (`tests/am1/results/b033-quarter.md`, a1's answered kinds).
fn kinds(n: usize) -> Vec<&'static str> {
    let share = [("push", 25), ("task", 30), ("comment", 12), ("label", 8), ("renew", 14), ("acquire", 6)];
    let mut out = Vec::new();
    for (k, pct) in share {
        let c = (n * pct + 50) / 100;
        out.extend(std::iter::repeat_n(k, if k == "push" { c.max(1) } else { c }));
    }
    while out.len() < n {
        out.push("pr");
    }
    out.truncate(n);
    out
}

struct Fixtures {
    by: BTreeMap<String, Value>,
}

impl Fixtures {
    fn new() -> Result<Fixtures> {
        let corpus: Value = serde_json::from_str(CORPUS)?;
        let mut by = BTreeMap::new();
        for e in corpus["events"].as_array().unwrap() {
            let k = e.as_object().and_then(|o| o.keys().next()).cloned().unwrap_or_default();
            by.entry(k).or_insert_with(|| e.clone());
        }
        Ok(Fixtures { by })
    }
    fn get(&self, k: &str) -> Value {
        self.by[k].clone()
    }
}

/// An issue command's `/commit` body as the edge sends it: the command and its ~10 KB render (LM-1 round 15b measured
/// 10,262 to 10,426 bytes of a 10.8 KB command).
fn issue_body(k: u64) -> Vec<u8> {
    let render: Map<String, Value> = (0..40).map(|i| (format!("field_{i}"), json!("x".repeat(240)))).collect();
    serde_json::to_vec(&json!({
        "rid": ulid(k),
        "cmd": { "issue": { "op": { "Comment": { "on": { "Issue": 1 }, "body": "a1", "reply_to": null } },
            "actor": { "actor": { "App": { "app": 3, "installation": 903 } } }, "request": ulid(k), "agent": true,
            "render": Value::Object(render) } },
        "admit": { "class": "Agent", "actor": 9223372036854775811u64, "push": false, "inline": 0, "placement_ok": true },
    }))
    .unwrap()
}

/// A push's `/commit` body with `pack` inline or stored: a real `RepoCmd::Push` as the edge serializes it (base64 of
/// its wire form), built as `crates/git/tests/commit_profile.rs` builds B0-33's.
fn push_body(k: u64, pack: graftyard_git::pack::PackRef) -> Vec<u8> {
    use graftyard_git::refs::{Atomicity, ObjectId, RefName, RefUpdate};
    use graftyard_identity::stateless::InstallationClaims;
    use graftyard_identity::{Access, AppId, InstallationId, Jti, Kid, PermissionSet, Principal, RepoScope};
    let names = ["contents", "issues", "pull_requests", "checks", "statuses", "metadata"];
    let claims = InstallationClaims {
        app: AppId(DbId(3)),
        installation: InstallationId(DbId(903)),
        repos: RepoScope::Selected(smallvec::smallvec![RepoId(DbId(7))]),
        permissions: PermissionSet(names.iter().map(|n| (Box::from(*n), Access::Write)).collect()),
        jti: Jti(Ulid(0x0193_0000_0000_0000_0000_0000_0000_0003)),
        iat: Timestamp(9_000),
        exp: Timestamp(9_000 + 3_600_000),
        kid: Kid(1),
        task: None,
    };
    let mut new = [7u8; 20];
    new[..8].copy_from_slice(&k.to_be_bytes());
    let name = RefName::parse(format!("refs/heads/claude/a1-{k}").as_bytes()).expect("a ref name");
    let push = graftyard_git::repo::Push {
        updates: vec![RefUpdate { name, expect: None, new: Some(ObjectId(new)) }],
        atomicity: Atomicity::PerRef,
        pack: Some(pack),
        actor: Principal::installation(&claims),
        options: Vec::new(),
        request: RequestId(Ulid(u128::from(k) + 0x0193_0000_0000_0000_0000_0000_0000)),
        agent: None,
        provenance: Vec::new(),
        protected: Vec::new(),
        changed: Vec::new(),
        leases: false,
    };
    serde_json::to_vec(&json!({ "rid": ulid(k), "cmd": RepoCmd::Push(push),
        "admit": { "class": "Agent", "actor": 9223372036854775811u64, "push": true, "inline": 0, "placement_ok": true } }))
    .unwrap()
}

fn commands(fx: &Fixtures, n: usize, base: u64, seq: u64, only: Option<&str>) -> Result<(Vec<Cmd>, Vec<&'static str>)> {
    let mut out = Vec::new();
    let mut inline_left = AGENT_INLINE_ENTRY;
    // `only` is a comma list: the mix's kinds in its order, kept to those listed, repeated to N.
    let mix: Vec<&str> = match only {
        Some(list) => {
            let keep: Vec<&str> = list.split(',').collect();
            let mut m: Vec<&str> = kinds(100).into_iter().filter(|k| keep.contains(k)).collect();
            if m.is_empty() {
                m = keep;
            }
            m.iter().cycle().take(n).copied().collect()
        }
        // The mix's shares over every 100 commands of the run, not per entry: at 11 an entry, rounding each kind's
        // share per entry would drop draft PRs (5%) altogether. `seq` is the run's count of commands before this entry.
        None => {
            let m = kinds(100);
            (0..n).map(|j| m[((seq + j as u64) * 37 % 100) as usize]).collect()
        }
    };
    for (j, kind) in mix.into_iter().enumerate() {
        let k = base + j as u64;
        let c = match kind {
            "push" => {
                let size = 10_000 + (k as usize * 7_919) % 60_000;
                let bytes = noise(size, k);
                let inline = size <= inline_left;
                let wire = if inline {
                    graftyard_git::pack::PackRef::Inline(bytes.clone())
                } else {
                    graftyard_git::pack::PackRef::Stored {
                        id: graftyard_git::pack::PackId(Ulid(u128::from(k) + 1_000_000_000)),
                        checksum: graftyard_types::Digest([3; 32]),
                        bytes: size as u64,
                        sites: graftyard_types::SiteSet(7),
                    }
                };
                let pack = if inline {
                    inline_left -= size;
                    json!({ "inline": b64(&bytes) })
                } else {
                    json!({ "stored": { "id": ulid(k + 1_000_000_000), "checksum": "@d:03", "bytes": size, "sites": 7 } })
                };
                let mut refs = fx.get("Refs");
                let r = &mut refs["Refs"];
                r["push"] = json!(ulid(k));
                r["updates"] =
                    json!([{ "name": format!("@ref:refs/heads/claude/a1-{k}"), "expect": null, "new": "@oid:00" }]);
                r["pack"] = pack.clone();
                let mut refs = expand(refs);
                refs["Refs"]["updates"][0]["new"] = oid(k);
                let mut prov = fx.get("Provenance");
                prov["Provenance"]["push"] = json!(ulid(k));
                let mut prov = expand(prov);
                prov["Provenance"]["list"][0]["commit"] = oid(k);
                prov["Provenance"]["list"][0]["refname"] =
                    Value::Array(format!("refs/heads/claude/a1-{k}").bytes().map(Value::from).collect());
                let body = push_body(k, wire);
                Cmd {
                    events: vec![
                        serde_json::from_value(refs).map_err(|e| Error::RustError(format!("Refs: {e}")))?,
                        serde_json::from_value(prov).map_err(|e| Error::RustError(format!("Provenance: {e}")))?,
                    ],
                    body,
                    push: true,
                    kind: "", k: 0,
                }
            }
            "task" => {
                let mut t = fx.get("TaskUpdated");
                t["TaskUpdated"]["summary"] = json!(format!("a1 {k}"));
                Cmd { events: vec![decode(t)?], body: issue_body(k), push: false, kind: "", k: 0 }
            }
            "comment" => {
                let mut c = fx.get("Comment");
                c["Comment"]["Created"]["id"] = json!(10_000 + k);
                Cmd { events: vec![decode(c)?], body: issue_body(k), push: false, kind: "", k: 0 }
            }
            "label" => Cmd { events: vec![decode(fx.get("Labels"))?], body: issue_body(k), push: false, kind: "", k: 0 },
            "renew" => {
                let mut r = fx.get("LeaseRenew");
                r["LeaseRenew"]["expires"] = json!(950_000_000 + k);
                Cmd { events: vec![decode(r)?], body: issue_body(k), push: false, kind: "", k: 0 }
            }
            "acquire" => {
                let mut a = fx.get("LeaseAcquire");
                // a1 gives tasks to at most 100 agents and each alternates acquire and release (`a1.mjs`, `side:lease`), so
                // at most about 100 leases are live: acquires reuse 100 ids.
                a["LeaseAcquire"]["lease"]["id"] = json!(ulid(k % 100 + 2_000_000_000));
                a["LeaseAcquire"]["lease"]["mode"] = json!("Shared");
                a["LeaseAcquire"]["plan"] = Value::Null;
                Cmd { events: vec![decode(a)?], body: issue_body(k), push: false, kind: "", k: 0 }
            }
            _ => {
                let mut p = fx.get("PrOpened");
                let n = 1_000 + k;
                p["PrOpened"]["n"] = json!(n);
                p["PrOpened"]["capture"]["issue"]["id"] = json!(100_000 + k);
                // a1 opens each draft PR on an agent's own pushed branch (`a1.mjs`, `side:draftPr`).
                p["PrOpened"]["head"]["branch"] = json!(format!("@ref:refs/heads/claude/a1-{}", k.saturating_sub(7)));
                Cmd { events: vec![decode(p)?], body: issue_body(k), push: false, kind: "", k: 0 }
            }
        };
        // The mix's own name for the kind, which outlives `only`.
        let kind = ["push", "task", "comment", "label", "renew", "acquire"].into_iter().find(|x| *x == kind).unwrap_or("pr");
        out.push(Cmd { kind, k, ..c });
    }
    Ok((out, Vec::new()))
}

fn entry(index: u64, prev: graftyard_types::Digest, cmds: &[Cmd], base: u64) -> LogEntry<RepoEvent> {
    let mut events = Vec::new();
    let mut requests = smallvec::SmallVec::new();
    for (j, c) in cmds.iter().enumerate() {
        let start = events.len() as u32;
        events.extend(c.events.iter().cloned());
        requests.push((RequestId(Ulid(u128::from(base + j as u64) + 1)), start..events.len() as u32));
    }
    let tiers = events.iter().fold(TierSet::NONE, |t, e| t.union(<RepoFold as Fold>::tiers(e)));
    LogEntry {
        term: Term(2),
        index: Index(index),
        prev,
        format: Format { family: 0x20, version: NEWEST },
        tiers,
        at: Timestamp(1_000 * index),
        requests,
        extend: None,
        body: EntryBody::Events(events),
    }
}

fn append(e: &LogEntry<RepoEvent>, to: u8, round: u64) -> Envelope<RepoEvent> {
    Envelope {
        from: SiteId(0),
        to: SiteId(to),
        msg: Message::Append(AppendReq {
            term: Term(2),
            leader: SiteId(0),
            boot: 1,
            round,
            sent_at: Timestamp(e.at.0),
            prev_index: Index(e.index.0 - 1),
            prev_term: Term(2),
            prev_digest: e.prev,
            entries: vec![e.clone()],
            commit: Index(e.index.0 - 1),
            transfer: None,
            lead_seq: round,
        }),
    }
}

fn i(v: u64) -> SqlStorageValue {
    SqlStorageValue::Integer(v as i64)
}

#[derive(serde::Deserialize)]
#[allow(dead_code)]
struct Req {
    rid: String,
    cmd: RepoCmd,
}

#[durable_object]
pub struct L {
    state: State,
    env: Env,
}

impl L {
    async fn prof(&self, url: &Url) -> Result<Value> {
        let q =
            |k: &str, d: u64| url.query_pairs().find(|(n, _)| n == k).and_then(|(_, v)| v.parse().ok()).unwrap_or(d);
        let (n, entries, peers, grow) = (q("cmds", 60) as usize, q("entries", 40), q("peers", 6) as u8, q("grow", 0));
        // `only=KIND` makes every command one kind of the mix, to split `fold_apply` by kind.
        let only = url.query_pairs().find(|(k, _)| k == "only").map(|(_, v)| v.into_owned());
        let sink = url.query_pairs().find(|(k, _)| k == "sink").map(|(_, v)| v.into_owned());
        let sql = self.state.storage().sql();
        for s in SCHEMA {
            sql.exec(s, None)?;
        }
        let corpus: Value = serde_json::from_str(CORPUS)?;
        let setup: Vec<RepoEvent> =
            corpus["setup"].as_array().unwrap().iter().cloned().map(decode).collect::<Result<_>>()?;
        let (core, meta) = tier::repo_tiers(&sql);
        // `memo=0`: the tiers read SQLite every time, as before `read-memo`.
        let memo = q("memo", 1) == 1;
        core.set_memo(memo);
        meta.set_memo(memo);
        let mut fold =
            RepoFold::with_tiers(Some(RepoId(DbId(7))), Some(AccountId(DbId(1))), Box::new(core), Box::new(meta));
        let first = LogEntry {
            term: Term(2),
            index: Index(1),
            prev: PREV_ZERO,
            format: Format { family: 0x20, version: NEWEST },
            tiers: setup.iter().fold(TierSet::NONE, |t, e| t.union(<RepoFold as Fold>::tiers(e))),
            at: Timestamp(1_000),
            requests: Default::default(),
            extend: None,
            body: EntryBody::Events(setup),
        };
        fold.apply_entry(&first);
        let mut stmts = Vec::new();
        tier::handle(&*fold.core).seal(&mut stmts);
        tier::handle(&*fold.meta).seal(&mut stmts);
        stmts.push((
            "INSERT OR REPLACE INTO log (idx, term, digest, row) VALUES (?, ?, ?, ?)",
            vec![i(1), i(2), graftyard_raft::digest(&first).0.to_vec().into(), codec::to_bytes(&first).into()],
        ));
        txn::commit_tx(&self.state.storage(), stmts).map_err(Error::RustError)?;
        tier::handle(&*fold.core).flushed();
        tier::handle(&*fold.meta).flushed();

        let fx = Fixtures::new()?;
        let owner = std::rc::Rc::new(owner_authz());
        let authz_bytes = codec::to_bytes(&*owner).len();
        let renders = Renders::new()?;
        let acked0 = ack::ack(());
        let mut ring = graftyard_git::feed::Ring::default();
        let mut by_kind: BTreeMap<&'static str, (f64, u64)> = BTreeMap::new();
        let mut issue_cmds = 0u64;
        let mut statuses: BTreeMap<String, u64> = BTreeMap::new();
        let mut t: BTreeMap<&str, f64> = STAGES.iter().map(|(s, _)| (*s, 0.0)).collect();
        let mut units: BTreeMap<&str, u64> = STAGES.iter().map(|(s, _)| (*s, 0)).collect();
        let (mut entry_bytes, mut stmts_n, mut stmt_bytes, mut pushes) = (0usize, 0usize, 0usize, 0usize);
        let (mut open_prs, mut lease_n) = (0usize, 0usize);
        let mut prev = graftyard_raft::digest(&first);
        let sinks: Vec<Stub> = (0..peers)
            .map(|p| self.env.durable_object("S")?.id_from_name(&format!("sink{p}"))?.get_stub())
            .collect::<Result<_>>()?;
        let ri = codec::to_bytes(&vec![Envelope::<RepoEvent> {
            from: SiteId(192),
            to: SiteId(0),
            msg: Message::ReadIndex { batch: 1 },
        }]);
        for x in 0..grow + entries {
            let idx = x + 2;
            let base = 1 + x * 1_000;
            let (cmds, _) = commands(&fx, n, base, x * n as u64, only.as_deref())?;
            // The first `grow` entries only build state (fold, tier rows, log, request rows) and are not timed.
            let timed = x >= grow;
            if timed {
                pushes += cmds.iter().filter(|c| c.push).count();
            }
            let mut add = |s: &'static str, a: f64, u: u64| {
                if timed {
                    *t.get_mut(s).unwrap() += now() - a;
                    *units.get_mut(s).unwrap() += u;
                }
            };

            let a = now();
            for c in &cmds {
                if c.push {
                    std::hint::black_box(
                        serde_json::from_slice::<Req>(&c.body).map_err(|e| Error::RustError(e.to_string()))?,
                    );
                } else {
                    std::hint::black_box(serde_json::from_slice::<Value>(&c.body)?);
                }
            }
            add("commit_decode", a, n as u64);

            let e = entry(idx, prev, &cmds, base);
            if timed {
                entry_bytes += graftyard_raft::encode_entry(&e).len();
            }

            let a = now();
            fold.apply_entry(&e);
            add("fold_apply", a, 1);

            let a = now();
            let mut stmts = Vec::new();
            stmts.push(("DELETE FROM request WHERE at < ?", vec![i(0)]));
            tier::handle(&*fold.core).seal(&mut stmts);
            tier::handle(&*fold.meta).seal(&mut stmts);
            add("seal", a, 1);

            let a = now();
            let d = graftyard_raft::digest(&e);
            stmts.push((
                "INSERT OR REPLACE INTO log (idx, term, digest, row) VALUES (?, ?, ?, ?)",
                vec![i(idx), i(2), d.0.to_vec().into(), codec::to_bytes(&e).into()],
            ));
            stmts.push(("INSERT OR REPLACE INTO raft_state (id, row) VALUES (1, ?)", vec![vec![9u8; 60].into()]));
            add("log_row", a, 1);

            let a = now();
            let EntryBody::Events(evs) = &e.body else { unreachable!() };
            for (rid, r) in &e.requests {
                let row: Vec<RepoEvent> =
                    evs[r.start as usize..r.end as usize].iter().map(<RepoFold as Fold>::row_event).collect();
                stmts.push((
                    txn::REQUEST_INSERT,
                    vec![rid.to_string().into(), i(idx), i(e.at.0), codec::to_bytes(&row).into()],
                ));
            }
            add("request_rows", a, n as u64);

            stmts_n += if timed { stmts.len() } else { 0 };
            stmt_bytes += if !timed {
                0
            } else {
                stmts
                    .iter()
                    .flat_map(|(_, a)| a.iter())
                    .map(|v| match v {
                        SqlStorageValue::Blob(b) => b.len(),
                        SqlStorageValue::String(s) => s.len(),
                        _ => 8,
                    })
                    .sum::<usize>()
            };
            let a = now();
            txn::commit_tx(&self.state.storage(), stmts).map_err(Error::RustError)?;
            tier::handle(&*fold.core).flushed();
            tier::handle(&*fold.meta).flushed();
            add("sql_tx", a, 1);
            if !timed {
                prev = d;
                continue;
            }

            let envs: Vec<Envelope<RepoEvent>> = (0..peers).map(|p| append(&e, p + 1, idx)).collect();
            let a = now();
            let bodies: Vec<Vec<u8>> = envs.iter().map(|env| codec::to_bytes(std::slice::from_ref(env))).collect();
            add("peer_encode", a, u64::from(peers));

            for (p, body) in bodies.iter().enumerate() {
                let a = now();
                let mut init = RequestInit::new();
                init.with_method(Method::Post).with_body(Some(js_sys::Uint8Array::from(&body[..]).into()));
                let req = Request::new_with_init(sink.as_deref().unwrap_or("https://member/raft"), &init)?;
                add("peer_send_js", a, 1);
                let a = now();
                // `sink=URL`: an outside HTTP sink, so workerd's process CPU holds the caller's side of each send only.
                let mut res = match &sink {
                    Some(_) => Fetch::Request(req).send().await?,
                    None => sinks[p].fetch_with_request(req).await?,
                };
                std::hint::black_box(res.bytes().await?);
                add("peer_send_wall", a, 1);
            }

            let held = append(&e, 192, idx);
            for c in 0..LEARNER_CALLS {
                let a = now();
                let got: Vec<Envelope<RepoEvent>> =
                    codec::from_bytes(&ri).map_err(|e| Error::RustError(e.to_string()))?;
                let mut back = vec![Envelope::<RepoEvent> {
                    from: SiteId(0),
                    to: got[0].from,
                    msg: Message::ReadIndexRes { batch: 1, index: Some((Index(idx), d)) },
                }];
                let with = c % 2 == 1;
                if with {
                    back.push(held.clone());
                }
                std::hint::black_box(Response::from_bytes(codec::to_bytes(&back))?);
                add(if with { "learner_call_append" } else { "learner_call_empty" }, a, 1);
            }

            for _ in 0..ENTRIES_CALLS {
                let a = now();
                let mut out = Vec::new();
                for r in sql
                    .exec(
                        "SELECT idx, row FROM log WHERE idx > ? ORDER BY idx LIMIT 2",
                        vec![i(idx.saturating_sub(2))],
                    )?
                    .raw()
                {
                    let r = r?;
                    let SqlStorageValue::Blob(row) = &r[1] else { continue };
                    let e: LogEntry<RepoEvent> = codec::from_bytes(row).map_err(|e| Error::RustError(e.to_string()))?;
                    let evs: &[RepoEvent] = match &e.body {
                        EntryBody::Events(evs) => evs,
                        _ => &[],
                    };
                    let mut m = Map::new();
                    m.insert("packs".into(), json!(graftyard_git::repo::packs_of(evs)));
                    m.insert("ref_updates".into(), json!(evs.iter().map(|ev| ev.updates().len()).sum::<usize>()));
                    m.insert("refs".into(), json!(graftyard_git::repo::entry_ref_changes(evs)));
                    m.insert("bytes".into(), json!(graftyard_raft::encode_entry(&e).len()));
                    m.insert("index".into(), json!(e.index.0));
                    m.insert("at".into(), json!(e.at.0));
                    out.push(Value::Object(m));
                }
                std::hint::black_box(serde_json::to_vec(&json!({ "entries": out }))?);
                add("entries_page", a, 1);
            }

            // The leader's per-entry consumer scans (`workers/site/src/prsync.rs` and `merge_queue.rs` `scan`): each
            // clones the entry from the member and asks which pull requests and queue bases it touched.
            let a = now();
            for _ in 0..2 {
                std::hint::black_box(e.clone());
            }
            add("consumer_clone", a, 1);
            let a = now();
            std::hint::black_box(graftyard_git::prsync::touched(&fold.core, RepoId(DbId(7)), &e));
            add("prsync_touched", a, 1);
            let a = now();
            std::hint::black_box(graftyard_git::queue_consumer::touched(&fold.core, RepoId(DbId(7)), &e));
            add("queue_touched", a, 1);

            // Two fold reads `decide` and the consumers repeat per command: `tiers::pulls_from` for a branch with no
            // pull request (each pushed ref asks it in apply, `prsync::touched`, `queue_consumer::touched` and a
            // push's `PathSet`), and `lease::rows`, every lease row (`LeaseView::live`, a lease acquire's caps).
            // A pushed branch with no pull request of its own that sorts among the open ones' heads, as each agent's
            // next branch does; and one that sorts after all of them.
            let fresh =
                graftyard_git::refs::RefName::parse(format!("refs/heads/claude/a1-{}x", base + 3).as_bytes()).unwrap();
            let after = graftyard_git::refs::RefName::parse(format!("refs/heads/claude/zz-{idx}").as_bytes()).unwrap();
            // Each probe read runs PROBE_REPS times, so the isolate's 1 ms clock resolves one call.
            let a = now();
            for _ in 0..PROBE_REPS {
                std::hint::black_box(graftyard_fold::Read::prefix(
                    &*fold.core,
                    "pull_by_head",
                    &[graftyard_fold::Val::Int(7), graftyard_git::tiers::ref_key(&after)],
                ));
            }
            add("pulls_from_after", a, PROBE_REPS);
            let a = now();
            for _ in 0..PROBE_REPS {
                // `tiers::pulls_from` is crate-private; this is its body.
                std::hint::black_box(graftyard_fold::Read::prefix(
                    &*fold.core,
                    "pull_by_head",
                    &[graftyard_fold::Val::Int(7), graftyard_git::tiers::ref_key(&fresh)],
                ));
            }
            add("pulls_from", a, PROBE_REPS);
            let a = now();
            for _ in 0..PROBE_REPS {
                std::hint::black_box(graftyard_git::lease::rows(&*fold.core));
            }
            add("lease_rows", a, PROBE_REPS);
            let a = now();
            for _ in 0..PROBE_REPS {
                std::hint::black_box(graftyard_fold::Read::scan(
                    &*fold.core,
                    "pull_by_head",
                    &[graftyard_fold::Val::Int(7)],
                    1,
                ));
            }
            add("scan_one", a, PROBE_REPS);
            let a = now();
            for _ in 0..PROBE_REPS {
                std::hint::black_box(graftyard_fold::Read::scan(
                    &*fold.core,
                    "pull_by_head",
                    &[graftyard_fold::Val::Int(7)],
                    256,
                ));
            }
            add("scan_256", a, PROBE_REPS);
            if x + 1 == grow + entries {
                open_prs = graftyard_fold::Read::prefix(&*fold.core, "pull_by_head", &[]).len();
                lease_n = graftyard_git::lease::rows(&*fold.core).len();
            }

            let a = now();
            for (rid, _) in &e.requests {
                std::hint::black_box(serde_json::to_vec(
                    &json!({ "done": { "ok": { "index": idx, "out": { "refs": [], "rid": rid.to_string() },
                    "quorum": [0, 1], "ack": { "index": idx, "acks": [{ "site": 0, "durable": idx, "at": 1 }, { "site": 1, "durable": idx, "at": 2 }] } } } }),
                )?);
            }
            add("reply_render", a, n as u64);

            // The leader's done replies as `workers/site` builds them, after the entry applied: each command's
            // `RepoOut` as `decide` returned it (an issue command's render the edge sent without the owner's `Authz`,
            // and that `Authz` as the entry's one shared `Rc`), the waiter's clone of it (`Host::release`), then
            // `Repo::out_json` (render decode, `fill_authz`, `render_ok` or `PushReport::of`), `outcome_json`,
            // `ack_json` and `Member::reply`'s `Response::from_json`, one per command.
            let outs: Vec<(&'static str, RepoOut)> = cmds.iter().map(|c| (c.kind, leader_out(c, &renders, &owner))).collect();
            let issues = outs.iter().filter(|(k, _)| *k != "push").count() as u64;
            if timed {
                issue_cmds += issues;
            }
            // Each stage over the entry's commands at once, so the isolate's 1 ms clock reads a sum, not a command.
            let a = now();
            let acks: Vec<graftyard_raft::Acked<RepoOut>> = outs.iter().map(|(_, o)| acked0.map(|_| o.clone())).collect();
            add("waiter_clone", a, n as u64);
            let a = now();
            let mut parsed: Vec<Option<graftyard_api::hosted::Render>> = acks
                .iter()
                .map(|x| match &x.out {
                    RepoOut::Issue(o) => serde_json::from_value(o.render.clone()).ok(),
                    _ => None,
                })
                .collect();
            add("render_parse", a, issues);
            let a = now();
            let sources: Vec<Option<&'static str>> = parsed
                .iter_mut()
                .zip(&acks)
                .map(|(r, x)| match (r, &x.out) {
                    (Some(r), RepoOut::Issue(o)) => graftyard_api::hosted::fill_authz(r, o.authz.as_deref()),
                    _ => None,
                })
                .collect();
            add("render_fill", a, issues);
            let mut replies: Vec<Value> = vec![Value::Null; n];
            for kind in ["push", "task", "comment", "label", "renew", "acquire", "pr"] {
                let a = now();
                let mut c = 0u64;
                for (j, x) in acks.iter().enumerate().filter(|(j, _)| outs[*j].0 == kind) {
                    replies[j] = out_json(&fold, x, parsed[j].as_ref(), sources[j]);
                    c += 1;
                    if timed {
                        let st = replies[j].pointer("/issue/reply/status").and_then(Value::as_u64).unwrap_or(0);
                        *statuses.entry(format!("{kind}:{st}")).or_default() += 1;
                    }
                }
                if timed && c > 0 {
                    let e = by_kind.entry(kind).or_default();
                    e.0 += now() - a;
                    e.1 += c;
                }
                add("render_ok", a, c);
            }
            let a = now();
            let built: Vec<Value> = acks
                .iter()
                .zip(replies)
                .map(|(x, reply)| {
                    let p = x.quorum();
                    json!({ "done": { "ok": {
                        "index": idx,
                        "out": reply,
                        "quorum": p.sites().iter().map(|s| s.0).collect::<Vec<_>>(),
                        "ack": {
                            "site": p.leader().0, "index": x.index.0, "commitIndex": p.index().0,
                            "entryAt": p.entry_at().0, "commitAt": p.at().0, "sentAt": now() as u64, "selfSyncedAt": idx,
                            "acks": p.acks().iter().map(|d| json!({ "site": d.site.0, "durable": d.durable.0, "at": d.at.0 }))
                                .collect::<Vec<_>>(),
                        },
                    } } })
                })
                .collect();
            add("reply_build", a, n as u64);
            let a = now();
            let mut sent = Vec::with_capacity(n);
            for mut v in built {
                v["meta"] = json!({ "term": 2, "leader": 0, "configIndex": 1 });
                sent.push(serde_json::to_string(&v)?);
                std::hint::black_box(Response::from_json(&v)?);
            }
            add("reply_send", a, n as u64);
            // A test install's `/commit` (`receipt_fault`, test-faults only): the answer's body read back as text,
            // parsed to name its kind, and sent again as bytes.
            let a = now();
            for body in sent {
                let v: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
                std::hint::black_box(v.get("done").is_some());
                std::hint::black_box(Response::from_bytes(body.into_bytes())?);
            }
            add("reply_reparse", a, n as u64);

            // The feed ring (`Repo::stepped`), which runs only on a member whose feed owner a poll has read: B0-33 runs
            // no pollers (`a1.mjs`, `pollers: 0`), so hosted it is skipped there. Priced as if one poll had read it.
            let a = now();
            let empty = graftyard_fold::MemStore::default();
            let meta: &dyn graftyard_fold::Read = if fold.meta.loaded() { &fold.meta } else { &empty };
            let EntryBody::Events(evs) = &e.body else { unreachable!() };
            ring.feed(RepoId(DbId(7)), &*owner, &fold.core, meta, (e.index, e.at, evs));
            add("feed_ring", a, 1);
            prev = d;
        }
        let e = entries as f64;
        let per_entry: Map<String, Value> =
            t.iter().map(|(k, v)| ((*k).to_owned(), json!((v / e * 100.0).round() / 100.0))).collect();
        let per_unit: Map<String, Value> = t
            .iter()
            .map(|(k, v)| ((*k).to_owned(), json!((v / units[k].max(1) as f64 * 1000.0).round() / 1000.0)))
            .collect();
        let unit: Map<String, Value> = STAGES.iter().map(|(k, u)| ((*k).to_owned(), json!(u))).collect();
        Ok(
            json!({ "cmds": n, "entries": entries, "grow": grow, "peers": peers, "openPrs": open_prs, "leases": lease_n, "pushesPerEntry": pushes as f64 / e,
            "entryBytes": entry_bytes as f64 / e, "stmtsPerEntry": stmts_n as f64 / e, "stmtBytesPerEntry": stmt_bytes as f64 / e,
            "learnerCallsPerEntry": LEARNER_CALLS, "authzBytes": authz_bytes, "memo": memo,
            "foldReads": [tier::handle(&*fold.core).read_counts(), tier::handle(&*fold.meta).read_counts()], "replyStatus": statuses, "issueCmds": issue_cmds,
            "renderByKind": by_kind.iter().map(|(k, (ms, c))| ((*k).to_owned(), json!({ "n": c, "msPer": ms / (*c).max(1) as f64 })))
                .collect::<Map<String, Value>>(), "entriesCallsPerEntry": ENTRIES_CALLS,
            "totalMs": t, "perEntryMs": per_entry, "perUnitMs": per_unit, "unit": unit }),
        )
    }
}

/// A repository member's two tiers over tables under `pre`, with the setup entry applied and flushed, its read memo
/// on or off (`read-memo`). No FTS5 rows: no route this bed checks reads them.
fn member(sql: &SqlStorage, storage: &Storage, pre: &'static str, memo: bool, first: &LogEntry<RepoEvent>) -> Result<RepoFold> {
    for schema in [graftyard_git::tiers::CORE, graftyard_git::tiers::METADATA] {
        for q in tier::ddl(&schema, pre) {
            sql.exec(&q, None)?;
        }
    }
    let core = tier::TierSql::new_in(sql.clone(), &graftyard_git::tiers::CORE, None, pre);
    let meta = tier::TierSql::new_in(sql.clone(), &graftyard_git::tiers::METADATA, None, pre);
    core.set_memo(memo);
    meta.set_memo(memo);
    let mut fold = RepoFold::with_tiers(Some(RepoId(DbId(7))), Some(AccountId(DbId(1))), Box::new(core), Box::new(meta));
    fold.apply_entry(first);
    flush(storage, &fold)?;
    Ok(fold)
}

/// The fold's buffered writes in one transaction, then `flushed`, as `host::flush` runs them.
fn flush(storage: &Storage, fold: &RepoFold) -> Result<()> {
    let mut stmts = Vec::new();
    tier::handle(&*fold.core).seal(&mut stmts);
    tier::handle(&*fold.meta).seal(&mut stmts);
    txn::commit_tx(storage, stmts).map_err(Error::RustError)?;
    tier::handle(&*fold.core).flushed();
    tier::handle(&*fold.meta).flushed();
    Ok(())
}

fn digests(fold: &RepoFold) -> (String, String) {
    use graftyard_types::Tier;
    (graftyard_fold::fold_digest(&*fold.core, Tier::Core).hex(), graftyard_fold::fold_digest(&*fold.meta, Tier::Metadata).hex())
}

/// Each command's done reply as `Repo::out_json` renders it from the fold after its entry, serialized.
fn replies(
    fold: &RepoFold,
    cmds: &[Cmd],
    renders: &Renders,
    owner: &std::rc::Rc<graftyard_identity::AuthzFold>,
) -> Vec<(&'static str, String)> {
    let acked0 = ack::ack(());
    cmds.iter()
        .map(|c| {
            let x = acked0.map(|_| leader_out(c, renders, owner));
            let mut parsed: Option<graftyard_api::hosted::Render> = match &x.out {
                RepoOut::Issue(o) => serde_json::from_value(o.render.clone()).ok(),
                _ => None,
            };
            let source = match (&mut parsed, &x.out) {
                (Some(r), RepoOut::Issue(o)) => graftyard_api::hosted::fill_authz(r, o.authz.as_deref()),
                _ => None,
            };
            (c.kind, out_json(fold, &x, parsed.as_ref(), source).to_string())
        })
        .collect()
}

impl L {
    /// `read-memo`'s check: a leader and a follower with the read memo on and a reference member with it off apply
    /// the same `entries` entries of B0-33's mix (or `only=K`), each flushing its own transaction. After every entry
    /// both memo members' fold digests must equal the reference's, and every reply the leader renders must be the
    /// reference's byte for byte. Answers the counts and the first mismatch, if any.
    fn memo_check(&self, url: &Url) -> Result<Value> {
        let q =
            |k: &str, d: u64| url.query_pairs().find(|(n, _)| n == k).and_then(|(_, v)| v.parse().ok()).unwrap_or(d);
        let (n, entries) = (q("cmds", 60) as usize, q("entries", 300));
        let only = url.query_pairs().find(|(k, _)| k == "only").map(|(_, v)| v.into_owned());
        let sql = self.state.storage().sql();
        let storage = self.state.storage();
        for s in SCHEMA {
            sql.exec(s, None)?;
        }
        let corpus: Value = serde_json::from_str(CORPUS)?;
        let setup: Vec<RepoEvent> =
            corpus["setup"].as_array().unwrap().iter().cloned().map(decode).collect::<Result<_>>()?;
        let first = LogEntry {
            term: Term(2),
            index: Index(1),
            prev: PREV_ZERO,
            format: Format { family: 0x20, version: NEWEST },
            tiers: setup.iter().fold(TierSet::NONE, |t, e| t.union(<RepoFold as Fold>::tiers(e))),
            at: Timestamp(1_000),
            requests: Default::default(),
            extend: None,
            body: EntryBody::Events(setup),
        };
        let mut leader = member(&sql, &storage, "a", true, &first)?;
        let mut follower = member(&sql, &storage, "f", true, &first)?;
        let mut reference = member(&sql, &storage, "r", false, &first)?;
        let fx = Fixtures::new()?;
        let owner = std::rc::Rc::new(owner_authz());
        let renders = Renders::new()?;
        let mut prev = graftyard_raft::digest(&first);
        let (mut compared, mut by_kind) = (0u64, BTreeMap::<&str, u64>::new());
        let mut mismatch: Option<Value> = None;
        for x in 0..entries {
            let idx = x + 2;
            let base = 1 + x * 1_000;
            let (cmds, _) = commands(&fx, n, base, x * n as u64, only.as_deref())?;
            let e = entry(idx, prev, &cmds, base);
            for f in [&mut leader, &mut follower, &mut reference] {
                f.apply_entry(&e);
            }
            // Read before the flush and after it, as the leader's consumer scans and the replies do.
            let mut pairs = Vec::new();
            pairs.push(("before flush", replies(&leader, &cmds, &renders, &owner), replies(&reference, &cmds, &renders, &owner)));
            for f in [&leader, &follower, &reference] {
                flush(&storage, f)?;
            }
            pairs.push(("after flush", replies(&leader, &cmds, &renders, &owner), replies(&reference, &cmds, &renders, &owner)));
            let want = digests(&reference);
            for (who, f) in [("leader", &leader), ("follower", &follower)] {
                if mismatch.is_none() && digests(f) != want {
                    mismatch = Some(json!({ "entry": idx, "member": who, "got": digests(f), "want": want }));
                }
            }
            for (when, got, want) in pairs {
                for ((kind, a), (_, b)) in got.iter().zip(&want) {
                    compared += 1;
                    *by_kind.entry(kind).or_default() += 1;
                    if mismatch.is_none() && a != b {
                        mismatch = Some(json!({ "entry": idx, "kind": kind, "when": when, "got": a, "want": b }));
                    }
                }
            }
            prev = graftyard_raft::digest(&e);
        }
        Ok(json!({
            "entries": entries, "cmds": n, "repliesCompared": compared, "byKind": by_kind, "mismatch": mismatch,
            "reads": {
                "leader": [tier::handle(&*leader.core).read_counts(), tier::handle(&*leader.meta).read_counts()],
                "follower": [tier::handle(&*follower.core).read_counts(), tier::handle(&*follower.meta).read_counts()],
                "reference": [tier::handle(&*reference.core).read_counts(), tier::handle(&*reference.meta).read_counts()],
            },
        }))
    }

    /// `read-memo`'s edge cases on real SQLite, each answering `true` when it holds:
    /// - `scratch`: `decide` reads a ref through the leader's scratch view, the scratch writes it, the scratch is
    ///   dropped, and both the next stored read and the next scratch's read see the stored value.
    /// - `buffered`: a ref read once, then written by `apply`, reads the new value before its transaction runs.
    /// - `flushed`: after the transaction runs, a ref written outside the handle's buffers by a later transaction (as
    ///   an install's table swap does) reads the new stored value.
    /// - `clear`: an undo table read once, then cleared, reads empty.
    /// - `refused`: a read SQLite refused is not kept: the next read of the row finds it.
    fn memo_scratch(&self) -> Result<Value> {
        use graftyard_fold::{Read, Store, Val};
        use graftyard_git::refs::RefName;
        use graftyard_raft::Scratch;
        let sql = self.state.storage().sql();
        let storage = self.state.storage();
        for s in SCHEMA {
            sql.exec(s, None)?;
        }
        let corpus: Value = serde_json::from_str(CORPUS)?;
        let setup: Vec<RepoEvent> =
            corpus["setup"].as_array().unwrap().iter().cloned().map(decode).collect::<Result<_>>()?;
        let first = LogEntry {
            term: Term(2),
            index: Index(1),
            prev: PREV_ZERO,
            format: Format { family: 0x20, version: NEWEST },
            tiers: setup.iter().fold(TierSet::NONE, |t, e| t.union(<RepoFold as Fold>::tiers(e))),
            at: Timestamp(1_000),
            requests: Default::default(),
            extend: None,
            body: EntryBody::Events(setup),
        };
        let mut fold = member(&sql, &storage, "s", true, &first)?;
        let main = RefName::parse(b"refs/heads/main").expect("a ref");
        let key = [graftyard_git::tiers::ref_key(&main)];
        let stored = |f: &RepoFold| Store::get(&f.core, "ref", &key).map(|r| r.cols);
        let fx = Fixtures::new()?;
        let refs_event = |new: u64| -> Result<RepoEvent> {
            let mut r = fx.get("Refs");
            r["Refs"]["updates"] = json!([{ "name": "@ref:refs/heads/main", "expect": null, "new": "@oid:00" }]);
            r["Refs"]["pack"] = Value::Null;
            let mut r = expand(r);
            r["Refs"]["updates"][0]["new"] = oid(new);
            serde_json::from_value(r).map_err(|e| Error::RustError(format!("Refs: {e}")))
        };
        let one = |idx: u64, prev, ev: RepoEvent| LogEntry {
            term: Term(2),
            index: Index(idx),
            prev,
            format: Format { family: 0x20, version: NEWEST },
            tiers: <RepoFold as Fold>::tiers(&ev),
            at: Timestamp(1_000 * idx),
            requests: Default::default(),
            extend: None,
            body: EntryBody::Events(vec![ev]),
        };
        let mut out = Map::new();

        // scratch
        let before = stored(&fold);
        let scratch_ok = {
            let mut s = Fold::scratch_at(&fold, Timestamp(5_000), TierSet(u8::MAX));
            let read = s.get(&main);
            s.apply(&refs_event(9_001)?);
            let wrote = s.get(&main);
            read.is_some() && wrote != read
        };
        let after = stored(&fold);
        let next_scratch = Fold::scratch_at(&fold, Timestamp(6_000), TierSet(u8::MAX)).core().get("ref", &key);
        out.insert("scratch".into(), json!(scratch_ok && before.is_some() && after == before && next_scratch == before));

        // buffered
        let mut prev = graftyard_raft::digest(&first);
        let _ = stored(&fold);
        let e2 = one(2, prev, refs_event(9_002)?);
        fold.apply_entry(&e2);
        let buffered = stored(&fold);
        out.insert("buffered".into(), json!(buffered.is_some() && buffered != before));
        flush(&storage, &fold)?;
        prev = graftyard_raft::digest(&e2);

        // flushed: a second handle on the same tables writes the row in its own transaction, as a load's or an
        // install's statements do, while this handle holds it in its memo.
        let held = stored(&fold);
        let mut other = member_handle(&sql, "s")?;
        let e3 = one(3, prev, refs_event(9_003)?);
        other.apply_entry(&e3);
        flush(&storage, &other)?;
        let now = stored(&fold);
        let theirs = stored(&other);
        out.insert("flushed".into(), json!(held.is_some() && now == theirs && now != held));

        // clear
        let undo = graftyard_fold::undo_table("ref");
        let mut h = tier::handle(&*fold.core).clone();
        // Read absent first, so the memo holds the absence when the snapshot writer's own statements (`crate::snap`,
        // run with `sql.exec`, not `commit_tx`) write the row and `flushed`.
        let absent = Read::get(&*fold.core, &undo, &[Val::Text("x".into())]).is_none();
        h.put(&undo, vec![Val::Text("x".into())], graftyard_fold::Stored { written: Index(1), cols: vec![Val::Int(1)] });
        let mut stmts = Vec::new();
        h.seal(&mut stmts);
        for (q, a) in stmts {
            sql.exec(q, a)?;
        }
        h.flushed();
        let read = Read::get(&*fold.core, &undo, &[Val::Text("x".into())]).is_some();
        h.clear(&undo);
        let gone = Read::get(&*fold.core, &undo, &[Val::Text("x".into())]).is_none();
        out.insert("snapFlushed".into(), json!(absent && read));
        out.insert("clear".into(), json!(read && gone));

        // prefixKey: two prefixes of one table read in the same transaction answer their own rows (`main` and
        // `feature`, both refs of the setup entry).
        let feature = RefName::parse(b"refs/heads/feature").expect("a ref");
        let pa = Read::prefix(&*fold.core, "ref", &[graftyard_git::tiers::ref_key(&main)]);
        let pb = Read::prefix(&*fold.core, "ref", &[graftyard_git::tiers::ref_key(&feature)]);
        out.insert(
            "prefixKey".into(),
            json!(pa.len() == 1 && pb.len() == 1 && pa[0].0 == key.to_vec() && pb[0].0 == vec![graftyard_git::tiers::ref_key(&feature)]),
        );

        // refused: on a fresh member, so the lost read marks nothing the other cases use.
        let lost = member(&sql, &storage, "t", true, &first)?;
        tier::handle(&*lost.core).fail_next_read();
        let failed = stored(&lost);
        let again = stored(&lost);
        out.insert("refused".into(), json!(failed.is_none() && again.is_some()));
        Ok(Value::Object(out))
    }
}

/// A second handle on a member's tables under `pre`, already set up, as a load's own handle is (`crate::load`).
fn member_handle(sql: &SqlStorage, pre: &'static str) -> Result<RepoFold> {
    let core = tier::TierSql::new_in(sql.clone(), &graftyard_git::tiers::CORE, None, pre);
    let meta = tier::TierSql::new_in(sql.clone(), &graftyard_git::tiers::METADATA, None, pre);
    Ok(RepoFold::with_tiers(Some(RepoId(DbId(7))), Some(AccountId(DbId(1))), Box::new(core), Box::new(meta)))
}

impl DurableObject for L {
    fn new(state: State, env: Env) -> Self {
        Self { state, env }
    }

    async fn fetch(&self, req: Request) -> Result<Response> {
        let url = req.url()?;
        match url.path() {
            "/prof" => Response::from_json(&self.prof(&url).await?),
            "/memo" => Response::from_json(&self.memo_check(&url)?),
            "/memo_scratch" => Response::from_json(&self.memo_scratch()?),
            _ => Response::error("not found", 404),
        }
    }
}

/// A peer that reads the AppendEntries body and answers a small 200, as a follower's `/raft` answer would be.
#[durable_object]
pub struct S {
    _state: State,
}

impl DurableObject for S {
    fn new(state: State, _env: Env) -> Self {
        Self { _state: state }
    }

    async fn fetch(&self, mut req: Request) -> Result<Response> {
        let b = req.bytes().await?;
        Response::from_bytes(vec![0u8; 64.min(b.len())])
    }
}

#[event(fetch)]
async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    let url = req.url()?;
    if url.path() == "/noop" {
        return Response::ok("{}");
    }
    let name = url.query_pairs().find(|(k, _)| k == "o").map(|(_, v)| v.to_string()).unwrap_or_default();
    if name.is_empty() {
        return Response::error("not found", 404);
    }
    env.durable_object("L")?.id_from_name(&name)?.get_stub()?.fetch_with_request(req).await
}

/// The repository owner's `Authz` fold, the size of m3org's: 94 members and 209 repository names, about 5 KB in the
/// codec (`tests/am1/results/lm1-memory.md`, round 15b, measured 4,987 bytes), with the grant and the installation a1's
/// App acts through.
fn owner_authz() -> graftyard_identity::AuthzFold {
    use graftyard_identity::{
        AccountKind, AuthzEvent, AuthzFold, InstallationId, InstallationRow, NameEpoch, OrgRole, RepoEntry, RepoName,
        RepoPermission, RepoScope, UserId,
    };
    let mut f = AuthzFold::new(AccountId(DbId(1)));
    let mut apply = |e: AuthzEvent| graftyard_raft::Fold::apply(&mut f, &e);
    apply(AuthzEvent::Opened { kind: AccountKind::Org, enterprise: None });
    for u in 0..94 {
        apply(AuthzEvent::Member { user: UserId(AccountId(DbId(1_000 + u))), role: Some(OrgRole::Member) });
    }
    for r in 0..209 {
        let key = RepoName::parse(&format!("a1-repo-{r}")).expect("a name").key();
        apply(AuthzEvent::RepoName { key, entry: Some(RepoEntry::Live { repo: RepoId(DbId(100 + r)), epoch: NameEpoch(1) }) });
    }
    for u in [5, 9] {
        apply(AuthzEvent::Grant { repo: RepoId(DbId(7)), grantee: UserId(AccountId(DbId(u))), permission: Some(RepoPermission::Write) });
    }
    let names = ["contents", "issues", "pull_requests", "checks", "statuses", "metadata"];
    let permissions = graftyard_identity::PermissionSet(
        names.iter().map(|n| (Box::from(*n), graftyard_identity::Access::Write)).collect(),
    );
    let row = InstallationRow {
        app: graftyard_identity::AppId(DbId(3)),
        repos: RepoScope::Selected(smallvec::smallvec![RepoId(DbId(7))]),
        permissions,
    };
    apply(AuthzEvent::Installation { id: InstallationId(DbId(903)), row: Some(row), not_before: Timestamp(0) });
    f
}

/// Each issue kind's render as the edge sends it on a first send (`commit_issue_rid`, `workers/edge/src/api.rs`):
/// no owner `Authz` (`leader-render`), the repository, and the logins its picks named.
struct Renders {
    by: BTreeMap<&'static str, Value>,
}

impl Renders {
    fn new() -> Result<Renders> {
        use graftyard_api::hosted::{HostedDir, Render, What};
        let dir = HostedDir {
            home: Some(graftyard_api::view::RepoRef {
                id: RepoId(DbId(7)),
                owner: "m3org".into(),
                owner_id: AccountId(DbId(1)),
                name: "r".into(),
            }),
            authz: None,
            logins: [(1, "m3org".to_owned()), (5, "u5".to_owned())].into_iter().collect(),
            ..HostedDir::default()
        };
        let gql = |query: &str, variables: Value, field: &str, subject: Option<u32>| What::Graphql {
            query: query.into(),
            variables,
            field: field.into(),
            subject,
        };
        let whats = [
            ("task", What::Task { created: false, contents: true }),
            (
                "comment",
                gql(
                    "mutation($i: AddCommentInput!) { addComment(input: $i) { subject { id } } }",
                    json!({ "i": { "subjectId": "I_kwDOAAAAAAAAAAc", "body": "a1" } }),
                    "addComment",
                    Some(1),
                ),
            ),
            ("label", What::IssueLabels { n: 1, empty: false }),
            ("renew", What::Lease { created: false }),
            ("acquire", What::Lease { created: true }),
            (
                "pr",
                gql(
                    "mutation($i: CreatePullRequestInput!) { createPullRequest(input: $i) { pullRequest { id number } } }",
                    json!({ "i": { "repositoryId": "R_x", "baseRefName": "main", "headRefName": "claude/a1-1",
                        "title": "claude/a1-1", "body": "a1", "draft": true } }),
                    "createPullRequest",
                    None,
                ),
            ),
        ];
        let mut by = BTreeMap::new();
        for (k, what) in whats {
            let r = Render { host: "graftyard.example".into(), isolation: false, dir: dir.clone(), what };
            by.insert(k, serde_json::to_value(&r)?);
        }
        Ok(Renders { by })
    }
}

/// A command's output as `decide` returns it for the mix's events: a push's ref outcome, or an issue command's
/// `IssueCallOut` with the edge's render and the entry's shared owner `Authz`.
fn leader_out(c: &Cmd, renders: &Renders, owner: &std::rc::Rc<graftyard_identity::AuthzFold>) -> RepoOut {
    use graftyard_git::issue_ops::IssueOut;
    use graftyard_git::issues::IssueNumber;
    let k = c.k;
    let out = match c.kind {
        "push" => {
            let name =
                graftyard_git::refs::RefName::parse(format!("refs/heads/claude/a1-{k}").as_bytes()).expect("a ref");
            return RepoOut::Push(graftyard_git::repo::PushOut { refs: vec![(name, graftyard_git::repo::RefOutcome::Ok)] });
        }
        "task" => IssueOut::Task(
            graftyard_identity::SessionId::parse("00000000-0000-0000-0000-000000000031", None).expect("a session"),
        ),
        "comment" => IssueOut::Id(DbId(10_000 + k as i64)),
        "label" => IssueOut::Done,
        "renew" => IssueOut::Lease(graftyard_git::lease::LeaseId::parse("00000000000000000000000021").expect("an id")),
        "acquire" => IssueOut::Lease(graftyard_git::lease::LeaseId::parse(&ulid(k % 100 + 2_000_000_000)).expect("an id")),
        _ => IssueOut::Number(IssueNumber(1_000 + k as u32)),
    };
    RepoOut::Issue(graftyard_git::issue_ops::call::IssueCallOut {
        out,
        render: renders.by[c.kind].clone(),
        authz: Some(owner.clone()),
    })
}

/// `Repo::out_json` (`workers/site/src/family.rs`) after its render decode and `fill_authz`, which the caller timed
/// apart: a push's `PushReport::of`, an issue command's `render_ok` over the member's tiers.
fn out_json(
    fold: &RepoFold,
    a: &graftyard_raft::Acked<RepoOut>,
    render: Option<&graftyard_api::hosted::Render>,
    source: Option<&'static str>,
) -> Value {
    match &a.out {
        RepoOut::Issue(o) => {
            let reply = match render {
                Some(r) => graftyard_api::hosted::render_ok(r, &a.map(|_| o.out.clone()), &fold.core, &fold.meta),
                None => graftyard_api::hosted::WireReply { status: 500, headers: Vec::new(), body: None },
            };
            let mut v = json!({ "issue": { "out": format!("{:?}", o.out), "reply": reply } });
            if let Some(s) = source {
                v["issue"]["authz"] = json!(s);
            }
            v
        }
        _ => serde_json::to_value(graftyard_git::group::PushReport::of(a)).unwrap_or(Value::Null),
    }
}
