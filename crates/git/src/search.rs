//! Cross-repository search (`design/DESIGN.md`, Shape, "Identity", `SearchShard` and `Domain`; `design/SHARED.md`,
//! `rest:search` and "Public service concerns", Residency; `build/BUILD-PLAN.md`, M4-WP2).
//!
//! - **Where a document lives.** `Domain::of` takes the owner's residency from its `PlacementRecord` name. `Search`
//!   writes each issue or pull request to `SearchShard { domain, shard: shard_of(repo), generation }`, 16 shards per
//!   domain by repository hash [guess]. A shard object is placed on the domain's first Full site that is not lost, under
//!   the domain's DO jurisdiction (`Domain::jurisdiction`), so an EU-resident document never leaves `eu` storage.
//! - **What a shard holds.** `DDL`: a plain `doc` table (repository, number, kind, state, visibility, times, and the
//!   comment count) and a contentless FTS5 table `doc_fts(title, body, comments)` with `contentless_delete=1` under the
//!   same rowid, the shape the hosted C15 probe confirmed in Durable Object SQLite (`tests/m4/c15`). `Doc::stmts`
//!   writes one document in one transaction, and `delete_stmts` removes it.
//! - **The `Search` consumer.** `docs_for` lists the documents one committed entry changes, read from the repository's
//!   metadata tier after the entry; `all_docs` lists every document, which a rebuild under the next `generation` writes
//!   from the metadata tiers `MemberRegistry` lists.
//! - **A query.** `parse` reads GitHub's syntax for the qualifiers this build serves; `fts_match` quotes every term,
//!   so a query cannot inject FTS5 syntax. `query_sql` runs on each shard of the `Default` domain and of the viewer's
//!   orgs' domains, filtered to public documents or the viewer's reach. `merge` combines the shards' answers; a shard
//!   that missed `SHARD_DEADLINE` is left out and the reply sets `incomplete_results`.

use std::time::Duration;

use graftyard_fold::Read;
use graftyard_raft::{EntryBody, LogEntry};
use graftyard_types::{AccountId, Jurisdiction, PlacementName, RepoId, SiteId, SiteTable, Timestamp};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::issues::{CommentEvent, CommentTarget, IssueKind, IssueNumber, Visibility};
use crate::repo::RepoEvent;
use crate::tiers::{CommentRow, IssueRow, get_rec, int, n_key};

/// Shards per residency domain [guess].
pub const SHARDS_PER_DOMAIN: u8 = 16;
/// A shard that has not answered by this time is left out and the reply sets `incomplete_results` [guess].
pub const SHARD_DEADLINE: Duration = Duration::from_secs(5);

/// The owner's residency domain, from its `PlacementRecord` name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Domain {
    Default,
    Us,
    Eu,
    Apac,
}

impl Domain {
    pub const ALL: [Domain; 4] = [Domain::Default, Domain::Us, Domain::Eu, Domain::Apac];

    pub fn of(placement: PlacementName) -> Domain {
        match placement {
            PlacementName::Us2w => Domain::Us,
            PlacementName::Eu2w => Domain::Eu,
            PlacementName::Apac2w => Domain::Apac,
            PlacementName::Tri | PlacementName::Na3 | PlacementName::Solo => Domain::Default,
        }
    }

    /// The DO jurisdiction a shard object is created under. `Apac` has none: Cloudflare offers `eu`, `us`, and
    /// `fedramp` only (`research/cloudflare.md`, Durable Objects, Jurisdictions), so APAC residency rests on placement.
    pub fn jurisdiction(self) -> Option<Jurisdiction> {
        match self {
            Domain::Eu => Some(Jurisdiction::Eu),
            Domain::Us => Some(Jurisdiction::Us),
            Domain::Default | Domain::Apac => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Domain::Default => "default",
            Domain::Us => "us",
            Domain::Eu => "eu",
            Domain::Apac => "apac",
        }
    }

    /// Whether site `s` may hold this domain's shards: a jurisdiction domain needs a site of that jurisdiction, and
    /// APAC a site in `apac` or `oc`. `Default` takes any active site.
    pub fn admits(self, s: &graftyard_types::Site) -> bool {
        use graftyard_types::Region;
        !s.spare
            && match self {
                Domain::Default => true,
                Domain::Eu | Domain::Us => s.jurisdiction == self.jurisdiction(),
                Domain::Apac => matches!(s.region, Region::Apac | Region::Oc),
            }
    }
}

/// DESIGN's `SearchShard`: one Durable Object per (domain, shard), renamed by `generation` when it is rebuilt.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SearchShard {
    pub domain: Domain,
    pub shard: u8,
    pub generation: u32,
}

impl SearchShard {
    pub fn do_name(&self) -> String {
        format!("search#{}#{}#{}", self.domain.name(), self.shard, self.generation)
    }

    /// The site the object lives on: the domain's first admitted site not in `lost`, in site order, skipping one more
    /// site for each generation after the first, so a rebuild lands on the domain's next site. `None` when the domain
    /// has no site left.
    pub fn site(&self, sites: &SiteTable, lost: &[SiteId]) -> Option<SiteId> {
        let ok: Vec<SiteId> =
            sites.0.iter().filter(|s| self.domain.admits(s) && !lost.contains(&s.id)).map(|s| s.id).collect();
        (!ok.is_empty()).then(|| ok[self.generation.saturating_sub(1) as usize % ok.len()])
    }
}

/// The shard a repository's documents go to.
pub fn shard_of(repo: RepoId) -> u8 {
    Sha256::digest(repo.0.0.to_be_bytes())[0] % SHARDS_PER_DOMAIN
}

// ---- documents ----

/// One issue or pull request as a shard stores it. `rowid` is `(repo << 24) | number`, as a thread id.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Doc {
    pub repo: RepoId,
    pub owner: AccountId,
    pub number: u32,
    pub kind: IssueKind,
    pub open: bool,
    pub public: bool,
    pub author: Option<AccountId>,
    pub title: Box<str>,
    pub body: Box<str>,
    /// Every comment's body, joined by newlines, so a term in a comment finds its issue, as GitHub's does.
    pub comments: Box<str>,
    pub comment_count: u32,
    pub created: Timestamp,
    pub updated: Timestamp,
}

pub fn rowid(repo: RepoId, number: u32) -> i64 {
    (repo.0.0 << 24) | i64::from(number & 0x00ff_ffff)
}

/// A SQL statement and its bound values, which the host runs on the shard's SQLite.
#[derive(Clone, Debug, PartialEq)]
pub struct Stmt {
    pub sql: &'static str,
    pub args: Vec<Arg>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Arg {
    Int(i64),
    Text(Box<str>),
}

/// The shard's tables. Contentless FTS5 with `contentless_delete=1`, as the metadata tier's (`workers/site/src/tier.rs`).
pub const DDL: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS doc (rowid INTEGER PRIMARY KEY, repo INTEGER NOT NULL, owner INTEGER NOT NULL, \
     number INTEGER NOT NULL, kind INTEGER NOT NULL, open INTEGER NOT NULL, public INTEGER NOT NULL, \
     author INTEGER, comments INTEGER NOT NULL, created INTEGER NOT NULL, updated INTEGER NOT NULL)",
    "CREATE INDEX IF NOT EXISTS doc_repo ON doc (repo)",
    "CREATE VIRTUAL TABLE IF NOT EXISTS doc_fts USING fts5(title, body, comments, content='', contentless_delete=1, \
     tokenize='porter unicode61 remove_diacritics 2')",
];

impl Doc {
    pub fn rowid(&self) -> i64 {
        rowid(self.repo, self.number)
    }

    /// Replaces the document: delete, then insert, so a re-sent write leaves one row.
    pub fn stmts(&self) -> Vec<Stmt> {
        let mut v = delete_stmts(self.repo, self.number);
        v.push(Stmt {
            sql: "INSERT INTO doc (rowid, repo, owner, number, kind, open, public, author, comments, created, updated) \
                  VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            args: vec![
                Arg::Int(self.rowid()),
                Arg::Int(self.repo.0.0),
                Arg::Int(self.owner.0.0),
                Arg::Int(i64::from(self.number)),
                Arg::Int(self.kind as i64),
                Arg::Int(i64::from(self.open)),
                Arg::Int(i64::from(self.public)),
                Arg::Int(self.author.map_or(0, |a| a.0.0)),
                Arg::Int(i64::from(self.comment_count)),
                Arg::Int(self.created.0 as i64),
                Arg::Int(self.updated.0 as i64),
            ],
        });
        v.push(Stmt {
            sql: "INSERT INTO doc_fts (rowid, title, body, comments) VALUES (?, ?, ?, ?)",
            args: vec![
                Arg::Int(self.rowid()),
                Arg::Text(self.title.clone()),
                Arg::Text(self.body.clone()),
                Arg::Text(self.comments.clone()),
            ],
        });
        v
    }
}

pub fn delete_stmts(repo: RepoId, number: u32) -> Vec<Stmt> {
    let id = rowid(repo, number);
    vec![
        Stmt { sql: "DELETE FROM doc WHERE rowid = ?", args: vec![Arg::Int(id)] },
        Stmt { sql: "DELETE FROM doc_fts WHERE rowid = ?", args: vec![Arg::Int(id)] },
    ]
}

/// Every document of a repository goes on a visibility or owner change, which a rebuild of its documents follows.
pub fn delete_repo_stmts(repo: RepoId) -> Vec<Stmt> {
    vec![
        Stmt {
            sql: "DELETE FROM doc_fts WHERE rowid IN (SELECT rowid FROM doc WHERE repo = ?)",
            args: vec![Arg::Int(repo.0.0)],
        },
        Stmt { sql: "DELETE FROM doc WHERE repo = ?", args: vec![Arg::Int(repo.0.0)] },
    ]
}

/// What the `Search` consumer knows of a repository besides its tiers: owner and visibility, from `Settings`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RepoInfo {
    pub repo: RepoId,
    pub owner: AccountId,
    pub public: bool,
}

impl RepoInfo {
    pub fn of(repo: RepoId, core: &(impl Read + ?Sized)) -> Option<RepoInfo> {
        let s: crate::issues::RepoSettings = get_rec(core, "settings", &[])?;
        Some(RepoInfo { repo, owner: s.owner, public: s.visibility == Visibility::Public })
    }
}

/// Issue `n`'s document as the metadata tier holds it, `None` once it is gone.
pub fn doc(info: RepoInfo, meta: &(impl Read + ?Sized), n: IssueNumber) -> Option<Doc> {
    let row: IssueRow = get_rec(meta, "issue", &[n_key(n)])?;
    let mut comments = String::new();
    for (k, _) in meta.prefix("comment_by_issue", &[n_key(n)]) {
        let Some(graftyard_fold::Val::Int(id)) = k.get(1) else { continue };
        if let Some(c) = get_rec::<CommentRow>(meta, "comment", &[int(*id)]) {
            comments.push_str(&c.body);
            comments.push('\n');
        }
    }
    Some(Doc {
        repo: info.repo,
        owner: info.owner,
        number: n.0,
        kind: row.kind,
        open: row.open,
        public: info.public,
        author: row.author,
        title: row.title,
        body: row.body,
        comments: comments.into(),
        comment_count: row.comments,
        created: row.created,
        updated: row.updated,
    })
}

/// What one committed entry asks of the index.
#[derive(Clone, Debug, PartialEq)]
pub enum DocOp {
    Put(Doc),
    Delete(u32),
    /// The repository's visibility or owner may have changed: rewrite all its documents.
    Repo,
}

/// The documents one committed entry changes, read from the tiers after it applied, in order.
pub fn docs_for(info: RepoInfo, meta: &(impl Read + ?Sized), e: &LogEntry<RepoEvent>) -> Vec<DocOp> {
    let EntryBody::Events(evs) = &e.body else { return Vec::new() };
    let mut ns: Vec<IssueNumber> = Vec::new();
    let mut repo = false;
    for ev in evs {
        let n = match ev {
            RepoEvent::IssueOpened { n, .. }
            | RepoEvent::PrOpened { n, .. }
            | RepoEvent::PrClosed { n, .. }
            | RepoEvent::IssueChanged { n, .. }
            | RepoEvent::Merged { n, .. } => Some(*n),
            RepoEvent::Comment(CommentEvent::Created { on: CommentTarget::Issue(n), .. }) => Some(*n),
            // An edit or delete names only the comment, whose row may be gone: find its issue by the index.
            RepoEvent::Comment(CommentEvent::Edited { id, .. } | CommentEvent::Deleted { id, .. }) => {
                comment_issue(meta, *id)
            }
            RepoEvent::Settings(_) | RepoEvent::Deleted { .. } => {
                repo = true;
                None
            }
            _ => None,
        };
        if let Some(n) = n
            && !ns.contains(&n)
        {
            ns.push(n);
        }
    }
    let mut out: Vec<DocOp> = Vec::new();
    if repo {
        out.push(DocOp::Repo);
    }
    for n in ns {
        out.push(match doc(info, meta, n) {
            Some(d) => DocOp::Put(d),
            None => DocOp::Delete(n.0),
        });
    }
    out
}

/// The issue a comment belonged to: the comment index keeps `(n, id)`, so a deleted comment's issue is found by scan.
fn comment_issue(meta: &(impl Read + ?Sized), id: crate::issues::CommentId) -> Option<IssueNumber> {
    if let Some(c) = get_rec::<CommentRow>(meta, "comment", &[int(id.0.0)]) {
        return crate::tiers::target_issue(&c.on);
    }
    meta.prefix("comment_by_issue", &[]).into_iter().find_map(|(k, _)| match (k.first(), k.get(1)) {
        (Some(graftyard_fold::Val::Int(n)), Some(graftyard_fold::Val::Int(c))) if *c == id.0.0 => {
            Some(IssueNumber(*n as u32))
        }
        _ => None,
    })
}

/// Every document of a repository, for `DocOp::Repo` and a shard rebuild. A deleted repository has none.
pub fn all_docs(info: RepoInfo, core: &(impl Read + ?Sized), meta: &(impl Read + ?Sized)) -> Vec<Doc> {
    if crate::tiers::deleted(core).is_some() {
        return Vec::new();
    }
    meta.prefix("issue", &[])
        .into_iter()
        .filter_map(|(k, _)| match k.first() {
            Some(graftyard_fold::Val::Int(n)) => doc(info, meta, IssueNumber(*n as u32)),
            _ => None,
        })
        .collect()
}

// ---- queries ----

/// GitHub's search syntax, for what this build serves: words, `"phrases"`, `is:`/`type:` issue, pr, open, closed,
/// `state:`, `in:title`/`in:body`/`in:comments`, `repo:`, `org:`/`user:`, and `author:`. The edge resolves the names
/// in `repo`, `owner`, and `author` to ids; an unknown one matches nothing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Query {
    pub terms: Vec<String>,
    pub kind: Option<IssueKind>,
    pub open: Option<bool>,
    pub columns: Vec<&'static str>,
    pub repo: Option<String>,
    pub owner: Option<String>,
    pub author: Option<String>,
    /// A qualifier this build does not serve: the edge answers 422 rather than a wrong result.
    pub unsupported: Option<String>,
}

pub fn parse(q: &str) -> Query {
    let mut out = Query::default();
    let mut rest = q.trim();
    while !rest.is_empty() {
        let (tok, phrase, next) = if let Some(r) = rest.strip_prefix('"') {
            let end = r.find('"').unwrap_or(r.len());
            (&r[..end], true, r.get(end + 1..).unwrap_or(""))
        } else {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            (&rest[..end], false, &rest[end..])
        };
        rest = next.trim_start();
        if tok.is_empty() {
            continue;
        }
        if !phrase && let Some((k, v)) = tok.split_once(':') {
            match (k.to_ascii_lowercase().as_str(), v.to_ascii_lowercase().as_str()) {
                ("is" | "type", "issue") => out.kind = Some(IssueKind::Issue),
                ("is" | "type", "pr" | "pull-request") => out.kind = Some(IssueKind::PullRequest),
                ("is" | "state", "open") => out.open = Some(true),
                ("is" | "state", "closed") => out.open = Some(false),
                ("in", cols) => {
                    for c in cols.split(',') {
                        match c {
                            "title" => out.columns.push("title"),
                            "body" => out.columns.push("body"),
                            "comments" => out.columns.push("comments"),
                            _ => out.unsupported = Some(tok.into()),
                        }
                    }
                }
                ("repo", _) => out.repo = Some(v.into()),
                ("org" | "user", _) => out.owner = Some(v.into()),
                ("author", _) => out.author = Some(v.into()),
                _ => out.unsupported = Some(tok.into()),
            }
            continue;
        }
        out.terms.push(tok.into());
    }
    out
}

/// The FTS5 `MATCH` text: each term double-quoted with its quotes doubled, so it is a string, never syntax, under
/// the `in:` column filter. `None` for a query with no words, which matches by its filters alone.
pub fn fts_match(q: &Query) -> Option<String> {
    if q.terms.is_empty() {
        return None;
    }
    let terms: Vec<String> = q.terms.iter().map(|t| format!("\"{}\"", t.replace('"', "\"\""))).collect();
    let body = terms.join(" AND ");
    Some(if q.columns.is_empty() { body } else { format!("{{{}}} : ({body})", q.columns.join(" ")) })
}

/// The ids the edge resolved a query's names to. `None` in a field means the qualifier was absent.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Resolved {
    pub repo: Option<Option<RepoId>>,
    pub owner: Option<Option<AccountId>>,
    pub author: Option<Option<AccountId>>,
}

/// Which private documents the viewer may see: every public one, plus those of these repositories.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reach(pub Vec<RepoId>);

/// The query each shard runs: matches newest-updated first for `sort=updated`, else best match (bm25), at most
/// `limit` rows, and the shard's total count of matches. Returns `None` when a resolved name matched nothing.
pub fn query_sql(
    q: &Query,
    r: &Resolved,
    reach: &Reach,
    sort_updated: bool,
    limit: usize,
) -> Option<(String, Vec<Arg>)> {
    let mut wh: Vec<String> = Vec::new();
    let mut args: Vec<Arg> = Vec::new();
    let fts = fts_match(q);
    if let Some(m) = &fts {
        wh.push("doc_fts MATCH ?".into());
        args.push(Arg::Text(m.as_str().into()));
    }
    if let Some(k) = q.kind {
        wh.push("d.kind = ?".into());
        args.push(Arg::Int(k as i64));
    }
    if let Some(o) = q.open {
        wh.push("d.open = ?".into());
        args.push(Arg::Int(i64::from(o)));
    }
    for (col, v) in [
        ("d.repo", r.repo.map(|x| x.map(|r| r.0.0))),
        ("d.owner", r.owner.map(|x| x.map(|a| a.0.0))),
        ("d.author", r.author.map(|x| x.map(|a| a.0.0))),
    ] {
        match v {
            None => {}
            Some(None) => return None,
            Some(Some(id)) => {
                wh.push(format!("{col} = ?"));
                args.push(Arg::Int(id));
            }
        }
    }
    let mut vis = "d.public = 1".to_string();
    if !reach.0.is_empty() {
        vis.push_str(&format!(" OR d.repo IN ({})", vec!["?"; reach.0.len()].join(", ")));
        args.extend(reach.0.iter().map(|r| Arg::Int(r.0.0)));
    }
    wh.push(format!("({vis})"));
    let (from, score) = match fts {
        Some(_) => ("doc_fts JOIN doc d ON d.rowid = doc_fts.rowid", "bm25(doc_fts)"),
        None => ("doc d", "0.0"),
    };
    let order = if sort_updated || score == "0.0" { "updated DESC, id DESC" } else { "score, updated DESC, id DESC" };
    // FTS5's `bm25()` refuses a window function in its own SELECT, so the score is taken in an inner query.
    let sql = format!(
        "SELECT repo, number, updated, score, count(*) OVER () AS total FROM (SELECT d.rowid AS id, d.repo, d.number, \
         d.updated, {score} AS score FROM {from} WHERE {}) ORDER BY {order} LIMIT {limit}",
        wh.join(" AND ")
    );
    Some((sql, args))
}

/// One match: where it is, and its sort keys.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub repo: RepoId,
    pub number: u32,
    pub updated: Timestamp,
    /// bm25: lower is better.
    pub score: f64,
}

/// One shard's answer: its top hits and its total, or `None` when it missed `SHARD_DEADLINE` or is lost.
pub type ShardAnswer = Option<(Vec<Hit>, u64)>;

/// The reply's page of hits, `total_count`, and `incomplete_results`.
#[derive(Clone, Debug, PartialEq)]
pub struct Merged {
    pub hits: Vec<Hit>,
    pub total: u64,
    pub incomplete: bool,
}

/// Merges the shards' answers for page `page` (from 1) of `per_page`. Each shard was asked for its top
/// `page * per_page`, so the merged page is exact over the shards that answered.
pub fn merge(answers: Vec<ShardAnswer>, sort_updated: bool, page: usize, per_page: usize) -> Merged {
    let incomplete = answers.iter().any(Option::is_none);
    let mut hits: Vec<Hit> = Vec::new();
    let mut total = 0;
    for (h, t) in answers.into_iter().flatten() {
        hits.extend(h);
        total += t;
    }
    let key = |h: &Hit| (h.updated, h.repo, h.number);
    if sort_updated {
        hits.sort_by_key(|h| std::cmp::Reverse(key(h)));
    } else {
        hits.sort_by(|a, b| a.score.total_cmp(&b.score).then_with(|| key(b).cmp(&key(a))));
    }
    let skip = page.saturating_sub(1).saturating_mul(per_page);
    Merged { hits: hits.into_iter().skip(skip).take(per_page).collect(), total, incomplete }
}

/// The shards a query reaches: all 16 of the `Default` domain and of each domain the viewer's orgs live in.
pub fn shards_for(domains: &[Domain], generation: impl Fn(Domain, u8) -> u32) -> Vec<SearchShard> {
    let mut ds: Vec<Domain> = vec![Domain::Default];
    for d in domains {
        if !ds.contains(d) {
            ds.push(*d);
        }
    }
    ds.into_iter()
        .flat_map(|domain| (0..SHARDS_PER_DOMAIN).map(move |shard| (domain, shard)))
        .map(|(domain, shard)| SearchShard { domain, shard, generation: generation(domain, shard) })
        .collect()
}
