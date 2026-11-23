//! AM2-WP2b's two coordination reads under `/api/graftyard/v1/repos/{owner}/{repo}` (`design/AGENT-LAYER.md`, journeys
//! A5 and A6, and "API reference"), which `graft-mcp`'s `coordination.overlaps` and `provenance.why` call:
//!
//! - `GET /overlaps?ref=&pr=&paths=`: the other refs whose `PathSet` (pushed or planned paths) overlaps the subject's,
//!   and the live leases that overlap it.
//! - `GET /provenance?ref=&path=&line=`: for the commit blame names for `line`, its `provenance` rows, the pull request
//!   it maps to, and that pull request's sessions.
//!
//! Neither table is in a whole-tier read (`crate::hosted::UNSERVED`), so both routes are narrow reads only
//! (`crate::narrow::Route::Overlaps` and `Route::Provenance`): the member runs the same functions over its whole fold
//! through a `Recorder`, with each selection over a whole table made in `Directory::pick`, and the edge renders from the
//! rows it recorded. Every row read here is one the edge then holds.

use graftyard_fold::{Key, Val};
use graftyard_git::agent::{PathSet, Source};
use graftyard_git::issues::{IssueNumber, PrState, PullHead};
use graftyard_git::lease::{LeaseRow, LeaseView};
use graftyard_git::refs::{ObjectId, RefName};
use graftyard_git::task::{RUN_PREFIX, task_core, task_meta};
use graftyard_git::tiers::{PullRow, agent_meta, get_rec, pull_head, ref_key, sha_key};
use graftyard_identity::glob::{PathGlob, glob_overlap};
use graftyard_identity::{AuthzRead, SessionId};
use graftyard_types::Timestamp;
use graftyard_wire::{FieldError, Reply};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use smallvec::SmallVec;

use crate::view::{Pick, Repo};

const DOC: &str = "graftyard/overlaps";
const PROVENANCE_DOC: &str = "graftyard/provenance";
/// The most overlapping refs one answer lists; past it `truncated` is true [guess: a page, as `per_page`'s maximum].
pub const OVERLAPS_MAX: usize = 100;
/// The most characters of a `Claimed` trailer's value an answer shows (`design/AGENT-LAYER.md`, journey A6).
pub const CLAIMED_MAX: usize = 200;
/// The pick keys (`crate::narrow`).
const PICK_PATH_SETS: &str = "overlaps.path_set";
const PICK_LEASES: &str = "overlaps.lease";
const PICK_PULL: &str = "provenance.pull";

fn invalid(resource: &'static str, doc: &str, field: &'static str, code: &'static str) -> Reply {
    let e = FieldError { resource, field, code };
    Reply::json(422, crate::wire::validation_error("Validation Failed", doc, &[e]))
}

fn query_pairs(query: &str) -> impl Iterator<Item = (&str, String)> {
    query
        .split('&')
        .filter(|kv| !kv.is_empty())
        .map(|kv| kv.split_once('=').unwrap_or((kv, "")))
        .map(|(k, v)| (k, crate::release::decode(v)))
}

/// A branch name as the API takes it (`main`, `feature/x`) or a full ref name.
fn full_ref(s: &str) -> Option<RefName> {
    let full = if s.starts_with("refs/") { s.to_owned() } else { format!("refs/heads/{s}") };
    RefName::parse(full.as_bytes()).ok()
}

// ---- overlaps ----

/// `GET /overlaps`'s subject: the `PathSet` of `ref`, the `PathSet` of pull request `pr`'s head ref, and the globs in
/// `paths`. It is also the narrow read's spec (`crate::narrow::Route::Overlaps`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverlapQuery {
    #[serde(rename = "ref")]
    pub refname: Option<String>,
    pub pr: Option<u32>,
    pub paths: Vec<String>,
}

/// `?ref=&pr=&paths=` with `paths` repeated (also as `paths[]`): 422 for a bad ref, pull request number, or glob,
/// more than FEED_PATHS_MAX `paths`, or none of the three.
pub fn parse_overlaps(query: &str) -> Result<OverlapQuery, Reply> {
    let bad = |f| invalid("Overlaps", DOC, f, "invalid");
    let mut q = OverlapQuery::default();
    for (k, v) in query_pairs(query) {
        match k {
            "ref" => q.refname = Some(full_ref(&v).ok_or_else(|| bad("ref"))?.display()),
            "pr" => q.pr = Some(v.parse::<u32>().ok().filter(|n| *n > 0).ok_or_else(|| bad("pr"))?),
            "paths" | "paths[]" | "paths%5B%5D" => {
                PathGlob::parse(&v).map_err(|_| bad("paths"))?;
                q.paths.push(v);
            }
            _ => {}
        }
    }
    if q.paths.len() > graftyard_git::feed::FEED_PATHS_MAX {
        return Err(bad("paths"));
    }
    if q.refname.is_none() && q.pr.is_none() && q.paths.is_empty() {
        return Err(invalid("Overlaps", DOC, "ref", "missing_field"));
    }
    Ok(q)
}

fn decode<T: serde::de::DeserializeOwned>(cols: &[Val]) -> Option<T> {
    match cols.first() {
        Some(Val::Blob(b)) => postcard::from_bytes(b).ok(),
        _ => None,
    }
}

/// Each glob a `PathSet` row names: its pushed paths (a `truncated` row's are directories ending in `/`, which
/// `glob_overlap` takes as everything under them) and its planned ones.
fn globs_of(p: &PathSet) -> impl Iterator<Item = &str> {
    p.paths.iter().chain(&p.planned).map(|s| &**s)
}

/// The subject's refs and globs.
fn subject(r: &Repo<'_>, q: &OverlapQuery) -> (Vec<RefName>, Vec<String>) {
    let mut refs: Vec<RefName> = q.refname.as_deref().and_then(full_ref).into_iter().collect();
    if let Some(h) = q.pr.and_then(|n| pull_head(r.core, IssueNumber(n)))
        && h.head.repo == r.at.id
        && !refs.contains(&h.head.branch)
    {
        refs.push(h.head.branch);
    }
    let mut globs = q.paths.clone();
    for rf in &refs {
        if let Some(p) = get_rec::<PathSet>(r.meta, "path_set", &[ref_key(rf)]) {
            globs.extend(globs_of(&p).map(str::to_owned));
        }
    }
    globs.sort_unstable();
    globs.dedup();
    (refs, globs)
}

fn overlapping<'a>(mine: &'a [String], theirs: impl Iterator<Item = &'a str>) -> Vec<&'a str> {
    let mut v: Vec<&str> = theirs.filter(|t| mine.iter().any(|m| glob_overlap(m, t))).collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// The open pull request whose head is `branch` of this repository: the row's own `pr`, else `pull_by_head`'s.
fn pull_of(r: &Repo<'_>, p: &PathSet) -> Option<IssueNumber> {
    p.pr.or_else(|| {
        r.core.prefix("pull_by_head", &[Val::Int(r.at.id.0.0), ref_key(&p.refname)]).into_iter().find_map(|(k, _)| {
            match k.get(2) {
                Some(Val::Int(n)) => u32::try_from(*n).ok().map(IssueNumber),
                _ => None,
            }
        })
    })
}

/// The session a Graftyard-run task's ref names: `refs/heads/graftyard/<uuid>/...`.
fn run_session(refname: &RefName) -> Option<SessionId> {
    let rest = refname.display();
    match SessionId::parse(rest.strip_prefix(RUN_PREFIX)?.split('/').next()?, None)? {
        s @ SessionId::Run(_) => Some(s),
        SessionId::External { .. } => None,
    }
}

fn overlap_json(r: &Repo<'_>, p: &PathSet, shared: Vec<&str>) -> Value {
    let pr = pull_of(r, p);
    let head = pr.and_then(|n| pull_head(r.core, n));
    let draft = head.map(|h| matches!(h.state, PrState::Open { draft: true }));
    // Who pushed it: the pull request's `AgentMeta`, else the live task whose prefix holds the ref.
    let (app, session, requester) = match pr.and_then(|n| agent_meta(r.core, n)) {
        Some(m) => (Some(m.app), m.session, m.requester),
        None => match run_session(&p.refname).and_then(|s| task_core(r.core, &s).map(|t| (s, t))) {
            Some((s, t)) => (Some(t.app), Some(s), t.requester),
            None => (None, None, None),
        },
    };
    json!({
        "ref": p.refname.display(),
        "pr": pr.map(|n| n.0),
        "draft": draft,
        "app": app.map(|a| a.0.0),
        "session": session.map(|s| s.text()),
        "requester": requester.map_or(Value::Null, |u| crate::wire::simple_user(r.host, r.dir, Some(u.0))),
        "last_push": p.head.hex(),
        "shared_paths": shared,
        "truncated": p.truncated,
        // `InsightVerdict` is AM4-WP2's: null is "no verdict yet", never "no conflict".
        "verdict": Value::Null,
    })
}

/// `GET /overlaps`'s body: `{overlaps, leases, truncated}`. `owner` is the owner's `Authz`, which decides which leases
/// are held; a member recording the read (`crate::narrow`) has none and reads every overlapping lease's rows.
pub fn overlaps_json(r: &Repo<'_>, owner: Option<&dyn AuthzRead>, at: Timestamp, q: &OverlapQuery) -> Value {
    let (refs, globs) = subject(r, q);
    let ref_texts: Vec<String> = refs.iter().map(RefName::display).collect();
    // The other refs' rows that overlap, in ref order, over the whole table.
    let mut select_rows = || {
        let mut items: Vec<Key> = Vec::new();
        for (k, cols) in r.meta.prefix("path_set", &[]) {
            let Some(p) = decode::<PathSet>(&cols) else { continue };
            if !refs.contains(&p.refname) && !overlapping(&globs, globs_of(&p)).is_empty() {
                items.push(k);
            }
        }
        let total = items.len();
        items.truncate(OVERLAPS_MAX);
        Pick { items, total, more: total > OVERLAPS_MAX }
    };
    let rows = r.dir.pick(PICK_PATH_SETS, &mut select_rows);
    let overlaps: Vec<Value> = rows
        .items
        .iter()
        .filter_map(|k| get_rec::<PathSet>(r.meta, "path_set", k))
        .map(|p| overlap_json(r, &p, overlapping(&globs, globs_of(&p))))
        .collect();
    // The leases whose paths or refs overlap, in grant order, then those held at `at`.
    let mut select_leases = || {
        let g: Vec<&str> = globs.iter().map(String::as_str).collect();
        let rf: Vec<&str> = ref_texts.iter().map(String::as_str).collect();
        let items: Vec<Key> = r
            .core
            .prefix("lease", &[])
            .into_iter()
            .filter(|(_, cols)| decode::<LeaseRow>(cols).is_some_and(|l| l.lease.overlaps(&g, &rf)))
            .map(|(k, _)| k)
            .collect();
        Pick { total: items.len(), items, more: false }
    };
    let leased = r.dir.pick(PICK_LEASES, &mut select_leases);
    let leases: Vec<Value> = leased
        .items
        .iter()
        .filter_map(|k| get_rec::<LeaseRow>(r.core, "lease", k))
        .filter(|row| match owner {
            Some(o) => LeaseView { core: r.core, owner: o, repo: r.at.id, at }.holds(row),
            // A recording member has no owner `Authz`: it reads the rows `holds` reads (the holder's task and a
            // plan-block lease's draft), so the edge holds them.
            None => {
                row.lease.holder.session.as_ref().map(|s| task_core(r.core, s));
                row.plan.map(|n| pull_head(r.core, n));
                true
            }
        })
        .map(|row| crate::leases::lease_json(r, &row, at))
        .collect();
    json!({ "overlaps": overlaps, "leases": leases, "truncated": rows.more })
}

/// `GET /overlaps`: 404 without `contents: read`, as every path field.
pub fn overlaps(r: &Repo<'_>, owner: &dyn AuthzRead, at: Timestamp, q: &OverlapQuery, contents: bool) -> Reply {
    if !contents {
        return Reply::not_found();
    }
    Reply::json(200, overlaps_json(r, Some(owner), at, q))
}

// ---- provenance ----

/// `GET /provenance`'s query: the ref blame runs at, the file, and the 1-based line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProvenanceQuery {
    pub refname: String,
    pub path: String,
    pub line: u32,
}

/// `?ref=&path=&line=`, all three required.
pub fn parse_provenance(query: &str) -> Result<ProvenanceQuery, Reply> {
    let (mut refname, mut path, mut line) = (None, None, None);
    for (k, v) in query_pairs(query) {
        match k {
            "ref" if !v.is_empty() => refname = Some(v),
            "path" if !v.is_empty() => path = Some(v.trim_start_matches('/').to_owned()),
            "line" => line = Some(v.parse::<u32>().ok().filter(|n| *n > 0)),
            _ => {}
        }
    }
    let missing = |f| invalid("Provenance", PROVENANCE_DOC, f, "missing_field");
    let refname = refname.ok_or_else(|| missing("ref"))?;
    let path = path.ok_or_else(|| missing("path"))?;
    let line =
        line.ok_or_else(|| missing("line"))?.ok_or_else(|| invalid("Provenance", PROVENANCE_DOC, "line", "invalid"))?;
    Ok(ProvenanceQuery { refname, path, line })
}

/// The commit the git cache's blame (`/ui/blame`: `{ranges: [{start, end, commit: {sha}}]}`) names for `line`, with
/// its range; 422 for a line past the file's end.
pub fn blamed(blame: &Value, line: u32) -> Result<(ObjectId, u32, u32), Reply> {
    let n = |v: &Value| v.as_u64().and_then(|x| u32::try_from(x).ok());
    blame["ranges"]
        .as_array()
        .into_iter()
        .flatten()
        .find_map(|g| {
            let (s, e) = (n(&g["start"])?, n(&g["end"])?);
            if !(s..=e).contains(&line) {
                return None;
            }
            ObjectId::parse_hex(g["commit"]["sha"].as_str()?.as_bytes()).map(|o| (o, s, e))
        })
        .ok_or_else(|| invalid("Provenance", PROVENANCE_DOC, "line", "invalid"))
}

fn source_json(r: &Repo<'_>, refname: &str, s: &Source) -> Value {
    match s {
        Source::Session(id) => json!({ "kind": "session", "id": id.text(), "linked": true, "ref": refname }),
        Source::Claimed { key, value } => {
            let shown: String = value.chars().take(CLAIMED_MAX).collect();
            let view = crate::trust::TrustView::of(r, &shown, None, false, false);
            json!({ "kind": "claimed", "key": trailer(*key), "value": view, "linked": false, "ref": refname })
        }
        Source::AiNote { note } => json!({ "kind": "ai_note", "note": note.hex(), "ref": refname }),
    }
}

fn trailer(k: graftyard_git::agent::TrailerKey) -> &'static str {
    use graftyard_git::agent::TrailerKey as T;
    match k {
        T::GraftyardSession => "Graftyard-Session",
        T::ClaudeSession => "Claude-Session",
        T::AmpThreadId => "Amp-Thread-ID",
        T::CoAuthoredBy => "Co-Authored-By",
        T::AssistedBy => "Assisted-By",
    }
}

/// The pull request `commit` maps to (journey A6, step 1): the one it merged (a squash or merge commit), else the one
/// whose head it is or was, else the newest one opened on a ref its `provenance` rows name, open or not.
fn select_pull(r: &Repo<'_>, commit: &ObjectId, refs: &[RefName]) -> Pick {
    let one = |n: u32| Pick::of_numbers([IssueNumber(n)], 1, false);
    let heads: Vec<(u32, PullHead)> = r
        .core
        .prefix("pull_head", &[])
        .into_iter()
        .filter_map(|(k, cols)| match k.first() {
            Some(Val::Int(n)) => Some((u32::try_from(*n).ok()?, decode::<PullHead>(&cols)?)),
            _ => None,
        })
        .collect();
    if let Some((n, _)) =
        heads.iter().find(|(_, h)| matches!(h.state, PrState::Merged { merge_commit, .. } if merge_commit == *commit))
    {
        return one(*n);
    }
    let was_head = |h: &PullHead| {
        h.head_sha == *commit
            || matches!(h.state, PrState::Merged { final_head, .. } | PrState::Closed { final_head, .. } if final_head == *commit)
    };
    if let Some((n, _)) = heads.iter().rev().find(|(_, h)| h.head.repo == r.at.id && was_head(h)) {
        return one(*n);
    }
    let by_ref = r.meta.prefix("pull", &[]).into_iter().rev().find_map(|(k, cols)| {
        let p = decode::<PullRow>(&cols)?;
        let n = match k.first() {
            Some(Val::Int(n)) => u32::try_from(*n).ok()?,
            _ => return None,
        };
        (p.head.repo == r.at.id && refs.contains(&p.head.branch)).then_some(n)
    });
    by_ref.map_or_else(Pick::default, one)
}

/// `GET /provenance`'s body for `commit`, with `lines` the blame range that holds the asked line (`None` while a member
/// records the read): `{commit, lines, pr, sessions, sources}`. A commit with no `provenance` row (a human's, or one
/// pushed before format 8) has no sources and whatever pull request the merge lookup finds.
pub fn provenance_json(r: &Repo<'_>, commit: ObjectId, lines: Option<(u32, u32)>) -> Value {
    let mut refs: Vec<RefName> = Vec::new();
    let mut sources: Vec<Value> = Vec::new();
    let mut sessions: Vec<SessionId> = Vec::new();
    for (k, cols) in r.meta.prefix("provenance", &[sha_key(&commit)]) {
        let Some(Val::Blob(b)) = k.get(1) else { continue };
        let Ok(rf) = RefName::parse(b) else { continue };
        let list: SmallVec<[Source; 2]> = decode(&cols).unwrap_or_default();
        for s in &list {
            if let Source::Session(id) = s
                && !sessions.contains(id)
            {
                sessions.push(id.clone());
            }
            sources.push(source_json(r, &rf.display(), s));
        }
        refs.push(rf);
    }
    let pick = r.dir.pick(PICK_PULL, &mut || select_pull(r, &commit, &refs));
    let pr = pick.numbers().first().copied().and_then(|n| pull_head(r.core, n).map(|h| (n, h)));
    if let Some(s) = pr.as_ref().and_then(|(n, _)| agent_meta(r.core, *n)).and_then(|m| m.session)
        && !sessions.contains(&s)
    {
        sessions.push(s);
    }
    let pr_json = pr.map_or(Value::Null, |(n, h)| {
        let (state, merged, draft) = match h.state {
            PrState::Open { draft } => ("open", false, draft),
            PrState::Closed { .. } => ("closed", false, false),
            PrState::Merged { .. } => ("closed", true, false),
        };
        json!({ "number": n.0, "state": state, "merged": merged, "draft": draft })
    });
    let sessions: Vec<Value> = sessions
        .iter()
        .map(|s| {
            let core = task_core(r.core, s);
            let meta = task_meta(r.meta, s);
            json!({
                "id": s.text(),
                "app": core.as_ref().map(|t| t.app.0.0),
                "requester": core.as_ref().and_then(|t| t.requester)
                    .map_or(Value::Null, |u| crate::wire::simple_user(r.host, r.dir, Some(u.0))),
                "issue": meta.as_ref().and_then(|m| m.issue).map(|n| n.0),
                "snapshot": meta.as_ref().map(|m| m.snapshot.0),
            })
        })
        .collect();
    json!({
        "commit": commit.hex(),
        "lines": lines.map(|(s, e)| json!({ "start": s, "end": e })),
        "pr": pr_json,
        "sessions": sessions,
        "sources": sources,
    })
}
