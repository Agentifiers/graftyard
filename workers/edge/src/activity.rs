//! `rest:activity` and `GET /search/issues` on the edge (`build/BUILD-PLAN.md`, M4-WP2; `design/SHARED.md`, "Surfaces,
//! tiers, and oracles"). The replies are `graftyard_api::activity`'s.
//!
//! - `GET /repos/{o}/{r}/stargazers` and `/subscribers` read the watcher index on the repository's metadata tier, which
//!   the `Counts` consumer writes. `GET`, `PUT`, and `DELETE /repos/{o}/{r}/subscription`, and `PUT` and
//!   `DELETE /user/starred/{o}/{r}`, read or commit `UserRecord`s on the caller's own `Identity` chain
//!   (`IdentityOp::SetRecord`, format 0x14.3), which is current at once; the counts follow through `Counts`.
//! - `GET /notifications` reads the caller's `Inbox` through its `Identity` leader (`/inbox`, with its `UserRows`) and
//!   renders each thread with its repository read on a follower. `PATCH /notifications/threads/{id}` marks a thread
//!   read and `DELETE` marks it done; `GET` and `PUT /notifications/threads/{id}/subscription` read and set the
//!   thread subscription. Each is a `SetRecord` on the caller's chain.
//! - `GET /search/issues` parses the query (`graftyard_git::search::parse`), runs `query_sql` on each shard of the
//!   `Default` domain and every domain the generation map names (`SearchShard` objects through the `SEARCH_<n>`
//!   bindings, under each domain's jurisdiction), and merges them. A shard that answers 503 or misses
//!   `SHARD_DEADLINE` is left out with `incomplete_results`.
//!
//! Until `FormatsEnable` (the edge's `FORMATS_ENABLED`) enabled both 32.19 and 0x14.3, every route here falls through
//! to the reply it had before (404), so no reply changes for a deployment that has not enabled them.

use std::collections::{BTreeMap, BTreeSet};

use graftyard_api::activity as act;
use graftyard_git::search::{self, Domain, SearchShard};
use graftyard_git::watch::{self, REPO_FORMAT_WATCH};
use graftyard_identity::{Actor, IDENTITY_FORMAT_V3, Principal, ThreadId, UserRecord, UserRows};
use graftyard_raft::codec;
use graftyard_types::{AccountId, ChainId, RepoId};
use graftyard_wire::{Host, Reply};
use serde_json::{Value, json};
use worker::*;

use crate::Ctx;
use crate::api::{Read, unavailable};

const DOC_NOTIFY: &str = "rest/activity/notifications";
const DOC_STAR: &str = "rest/activity/starring";

pub fn enabled(c: &Ctx<'_>) -> bool {
    c.cfg.enabled.contains(&REPO_FORMAT_WATCH) && c.cfg.enabled.contains(&IDENTITY_FORMAT_V3)
}

fn me(who: Option<&Principal>) -> std::result::Result<AccountId, Reply> {
    match who.map(|w| w.actor) {
        Some(Actor::User(u)) => Ok(u.0),
        Some(_) => Err(Reply::error(403, "Resource not accessible by integration", DOC_NOTIFY)),
        None => Err(Reply::error(401, "Requires authentication", "rest")),
    }
}

/// The caller's `UserRows`, through its `Identity` leader.
async fn user_rows(c: &Ctx<'_>, a: AccountId) -> std::result::Result<UserRows, Reply> {
    let record = match crate::placement(c, a).await {
        Ok(Some(r)) => r,
        Ok(None) => return Ok(UserRows::default()),
        Err(e) => return Err(unavailable(&e)),
    };
    let body = json!({ "q": { "user": true } }).to_string();
    let v = match graftyard_lookup::call_opt(&c.bindings(), ChainId::Identity(a), &record, "/read", body.as_bytes())
        .await
    {
        Ok(Some((v, _, _))) => v,
        Ok(None) => return Ok(UserRows::default()),
        Err(e) => return Err(unavailable(&e)),
    };
    let hex =
        v.pointer("/value/user").and_then(Value::as_str).ok_or_else(|| unavailable("no user rows (site release)"))?;
    codec::from_bytes(&graftyard_lookup::unhex(hex).unwrap_or_default()).map_err(|e| unavailable(&e.to_string()))
}

/// `SetRecord` on the caller's own chain; `Err` is the reply to send.
async fn set_record(c: &Ctx<'_>, who: &Principal, a: AccountId, r: UserRecord) -> std::result::Result<(), Reply> {
    let cmd = json!({ "cred": who.credential_shard(), "op": { "SetRecord": r } });
    match crate::ident::commit_owner(c, ChainId::Identity(a), a, cmd).await {
        Ok(d) if d.get("ok").is_some() => Ok(()),
        Ok(d) => Err(Reply::error(403, &format!("refused: {}", d["refused"]["err"]), DOC_NOTIFY)),
        Err(r) => Err(r),
    }
}

/// The routes under `/repos/{o}/{r}` this module serves, or `None` for another tail or before both formats.
pub async fn repo_route(
    c: &Ctx<'_>,
    req: &mut Request,
    rd: &Read,
    who: Option<&Principal>,
    tail: &[&str],
    query: &str,
    host: &Host,
) -> Result<Option<Reply>> {
    if !enabled(c) || !matches!(tail, ["stargazers"] | ["subscribers"] | ["subscription"]) {
        return Ok(None);
    }
    // A list page names accounts the repository's own rows do not, so their logins are read here, every shard the
    // page needs: at most `per_page` accounts.
    let mut dir = rd.dir.clone();
    if req.method() == Method::Get && tail != ["subscription"] {
        let page = graftyard_wire::Page::parse(query);
        let all: Vec<AccountId> = match tail {
            ["stargazers"] => watch::stargazers(&rd.meta).into_iter().map(|(a, _)| a).collect(),
            _ => watch::subscribers(&rd.meta),
        };
        let named: BTreeSet<AccountId> = all
            .into_iter()
            .skip(page.skip())
            .take(page.per_page)
            .filter(|a| !dir.logins.contains_key(&a.0.0))
            .collect();
        dir.logins.extend(crate::api::account_logins(c, &named, usize::MAX).await);
    }
    let r = graftyard_api::view::Repo { host, at: rd.at.clone(), core: &rd.core, meta: &rd.meta, dir: &dir };
    let method = req.method();
    Ok(Some(match (method, tail) {
        (Method::Get, ["stargazers"]) => {
            let accept = req.headers().get("accept")?.unwrap_or_default();
            act::stargazers(&r, query, &accept)
        }
        (Method::Get, ["subscribers"]) => act::subscribers(&r, query),
        (m @ (Method::Get | Method::Put | Method::Delete), ["subscription"]) => {
            let a = match me(who) {
                Ok(a) => a,
                Err(x) => return Ok(Some(x)),
            };
            if m != Method::Get {
                let b: Value = serde_json::from_slice(&req.bytes().await?).unwrap_or(Value::Null);
                let (subscribed, ignored) = if m == Method::Delete {
                    (false, false)
                } else {
                    (b["subscribed"].as_bool().unwrap_or(true), b["ignored"].as_bool().unwrap_or(false))
                };
                let rec = UserRecord::Watch { repo: rd.repo.id, subscribed, ignored };
                if let Err(x) = set_record(c, who.expect("me"), a, rec).await {
                    return Ok(Some(x));
                }
                if m == Method::Delete {
                    return Ok(Some(Reply::empty(204)));
                }
            }
            match user_rows(c, a).await {
                Ok(u) => act::repo_subscription(&r, &u),
                Err(x) => x,
            }
        }
        _ => return Ok(None),
    }))
}

/// `/user/starred/{o}/{r}`, `/notifications...`, and `/search/issues`, or `None` for another path or before both
/// formats.
pub async fn route(c: &Ctx<'_>, req: &mut Request, host: &Host, segs: &[&str], query: &str) -> Result<Option<Reply>> {
    if !enabled(c) {
        return Ok(None);
    }
    let method = req.method();
    let routed = matches!(
        (&method, segs),
        (Method::Get | Method::Put | Method::Delete, ["user", "starred", _, _])
            | (Method::Get, ["notifications"])
            | (_, ["notifications", "threads", _] | ["notifications", "threads", _, "subscription"])
            | (Method::Get, ["search", "issues"])
    );
    if !routed {
        return Ok(None);
    }
    let who = crate::authenticate(c, req).await?;
    if who.is_none() && crate::token_of(req)?.is_some() {
        return Ok(Some(Reply::error(401, "Bad credentials", "rest")));
    }
    if segs == ["search", "issues"] {
        return Ok(Some(search_issues(c, host, who.as_ref(), query).await?));
    }
    let a = match me(who.as_ref()) {
        Ok(a) => a,
        Err(x) => return Ok(Some(x)),
    };
    let who = who.expect("me");
    Ok(Some(match (method, segs) {
        (m, ["user", "starred", o, r]) => {
            let rd = match crate::api::read_core(c, o, r, Some(&who)).await? {
                Ok(rd) => rd,
                Err(x) => return Ok(Some(x)),
            };
            if m == Method::Get {
                let u = match user_rows(c, a).await {
                    Ok(u) => u,
                    Err(x) => return Ok(Some(x)),
                };
                return Ok(Some(if u.stars.contains_key(&rd.repo.id) {
                    Reply::empty(204)
                } else {
                    Reply::error(404, "Not Found", DOC_STAR)
                }));
            }
            match set_record(c, &who, a, UserRecord::Star { repo: rd.repo.id, on: m == Method::Put }).await {
                Ok(()) => Reply::empty(204),
                Err(x) => x,
            }
        }
        (Method::Get, ["notifications"]) => notifications(c, host, a, query).await?,
        (m, ["notifications", "threads", id]) => {
            let Some(t) = act::thread_id(id) else { return Ok(Some(Reply::not_found())) };
            match m {
                Method::Patch | Method::Delete => {
                    let rec = UserRecord::Thread { thread: t, read: true, done: m == Method::Delete };
                    match set_record(c, &who, a, rec).await {
                        Ok(()) => Reply::empty(if m == Method::Patch { 205 } else { 204 }),
                        Err(x) => x,
                    }
                }
                Method::Get => thread_get(c, host, a, t).await?,
                _ => Reply::not_found(),
            }
        }
        (m, ["notifications", "threads", id, "subscription"]) => {
            let Some(t) = act::thread_id(id) else { return Ok(Some(Reply::not_found())) };
            if m == Method::Put || m == Method::Delete {
                let b: Value = serde_json::from_slice(&req.bytes().await?).unwrap_or(Value::Null);
                let subscribed = m == Method::Put && !b["ignored"].as_bool().unwrap_or(false);
                if let Err(x) = set_record(c, &who, a, UserRecord::Subscription { thread: t, subscribed }).await {
                    return Ok(Some(x));
                }
                if m == Method::Delete {
                    return Ok(Some(Reply::empty(204)));
                }
            }
            match user_rows(c, a).await {
                Ok(u) => Reply::json(200, act::thread_subscription(&host.api(), t, &u)),
                Err(x) => x,
            }
        }
        _ => Reply::not_found(),
    }))
}

/// The caller's `Inbox` and `UserRows`, through its `Identity` leader's `/inbox`.
async fn inbox(c: &Ctx<'_>, a: AccountId) -> std::result::Result<(graftyard_git::inbox::Inbox, UserRows), Reply> {
    let record = match crate::placement(c, a).await {
        Ok(Some(r)) => r,
        Ok(None) => return Err(Reply::not_found()),
        Err(e) => return Err(unavailable(&e)),
    };
    let body = json!({ "op": "view" }).to_string();
    let v = match graftyard_lookup::call_opt(&c.bindings(), ChainId::Identity(a), &record, "/inbox", body.as_bytes())
        .await
    {
        Ok(Some((v, _, _))) => v,
        Ok(None) => return Err(unavailable("the account's group has no leader; retry")),
        Err(e) => return Err(unavailable(&e)),
    };
    let dec = |k: &str| graftyard_lookup::unhex(v[k].as_str().unwrap_or_default()).unwrap_or_default();
    let ib = codec::from_bytes(&dec("inbox")).map_err(|e| unavailable(&format!("inbox: {e}")))?;
    let u = codec::from_bytes(&dec("user")).map_err(|e| unavailable(&format!("user rows: {e}")))?;
    Ok((ib, u))
}

/// Each repository a page names, read on a follower once, for `thread_json`.
async fn repos_for(c: &Ctx<'_>, ids: BTreeSet<RepoId>, who: AccountId) -> BTreeMap<RepoId, Read> {
    let viewer = Principal {
        actor: Actor::User(graftyard_identity::UserId(who)),
        scopes: graftyard_identity::ScopeSet(0),
        credential: graftyard_identity::Credential::Stored([0; 32]),
        sso_orgs: Default::default(),
    };
    let reads = ids.into_iter().map(|id| {
        let viewer = viewer.clone();
        async move {
            let repo = crate::repo_by_id(c, id).await.ok()?;
            let (owner, name, repo, a) = crate::api::repo_named(c, repo).await?;
            let rd = crate::api::read_repo(
                c,
                repo,
                &owner,
                &name,
                Some(&viewer),
                Some(a),
                crate::api::Rows::All,
                crate::api::Gate::Readers,
            )
            .await
            .ok()?
            .ok()?;
            Some((id, rd))
        }
    });
    futures_util::future::join_all(reads).await.into_iter().flatten().collect()
}

async fn notifications(c: &Ctx<'_>, host: &Host, a: AccountId, query: &str) -> Result<Reply> {
    let q = match act::NotificationQuery::parse(query) {
        Ok(q) => q,
        Err(x) => return Ok(x),
    };
    let (ib, u) = match inbox(c, a).await {
        Ok(x) => x,
        Err(x) => return Ok(x),
    };
    let (page, more) = act::notification_page(&ib, &u, &q, None, query);
    let repos = repos_for(c, page.iter().map(|l| l.thread.repo).collect(), a).await;
    let items: Vec<Value> =
        page.iter().filter_map(|l| repos.get(&l.thread.repo).map(|rd| act::thread_json(&rd.view(host), l))).collect();
    let reply = Reply::json(200, Value::Array(items));
    Ok(if more {
        let p = graftyard_wire::Page::parse(query);
        reply.with("link", format!("<{}/notifications?page={}>; rel=\"next\"", host.api(), p.page + 1))
    } else {
        reply
    })
}

async fn thread_get(c: &Ctx<'_>, host: &Host, a: AccountId, t: ThreadId) -> Result<Reply> {
    let (ib, u) = match inbox(c, a).await {
        Ok(x) => x,
        Err(x) => return Ok(x),
    };
    let q = act::NotificationQuery { all: true, ..Default::default() };
    let all = ib.view(&u, q.all, false, Some(t.repo));
    let Some(l) = all.into_iter().find(|l| l.thread == t) else { return Ok(Reply::not_found()) };
    let repos = repos_for(c, [t.repo].into(), a).await;
    Ok(match repos.get(&t.repo) {
        Some(rd) => Reply::json(200, act::thread_json(&rd.view(host), &l)),
        None => Reply::not_found(),
    })
}

/// `GET /search/issues?q=`.
async fn search_issues(c: &Ctx<'_>, host: &Host, who: Option<&Principal>, query: &str) -> Result<Reply> {
    let raw = query.split('&').find_map(|kv| kv.strip_prefix("q=")).unwrap_or_default();
    let q_text =
        url::form_urlencoded::parse(format!("q={raw}").as_bytes()).find(|(k, _)| k == "q").map(|(_, v)| v.into_owned());
    let q_text = q_text.unwrap_or_default();
    let parsed = search::parse(&q_text);
    if parsed.unsupported.is_some() {
        return Ok(act::search_unsupported(&q_text));
    }
    let page = graftyard_wire::Page::parse(query);
    let sort_updated = query.split('&').any(|kv| kv == "sort=updated");
    // Names in the query resolve to ids; an unknown one matches nothing.
    let mut resolved = search::Resolved::default();
    if let Some(o) = &parsed.owner {
        resolved.owner = Some(crate::ident::account_of(c, o).await.map_err(Error::RustError)?.map(|(a, _)| a));
    }
    if let Some(o) = &parsed.author {
        resolved.author = Some(crate::ident::account_of(c, o).await.map_err(Error::RustError)?.map(|(a, _)| a));
    }
    if let Some(full) = &parsed.repo {
        let id = match full.split_once('/') {
            Some((o, n)) => crate::resolve(c, o, n).await?.ok().flatten().map(|r| r.id),
            None => None,
        };
        resolved.repo = Some(id);
    }
    // A private document only within the caller's reach: the repositories its orgs and grants name (here, the
    // repository a `repo:` qualifier names, when the caller may read it).
    let mut reach = Vec::new();
    if let (Some(Some(_)), Some(w)) = (resolved.repo, who)
        && let Some((o, n)) = parsed.repo.as_deref().and_then(|f| f.split_once('/'))
        && let Ok(Ok(rd)) = crate::api::read_core(c, o, n, Some(w)).await
    {
        reach.push(rd.repo.id);
    }
    let limit = page.end().min(1_000);
    let Some((sql, args)) = search::query_sql(&parsed, &resolved, &search::Reach(reach), sort_updated, limit) else {
        return Ok(act::search_reply(&search::Merged { hits: vec![], total: 0, incomplete: false }, vec![]));
    };
    let args: Vec<Value> = args
        .iter()
        .map(|a| match a {
            search::Arg::Int(i) => json!(i),
            search::Arg::Text(t) => json!(t.as_ref()),
        })
        .collect();
    let map = shard_map(c).await;
    let domains: Vec<Domain> = map.keys().map(|(d, _)| *d).collect();
    let shards = search::shards_for(&domains, |d, s| map.get(&(d, s)).copied().unwrap_or(1));
    let body = json!({ "sql": sql, "args": args }).to_string();
    let asks = shards.iter().map(|k| ask(c, k, &body));
    let answers: Vec<search::ShardAnswer> = futures_util::future::join_all(asks).await;
    let m = search::merge(answers, sort_updated, page.page, page.per_page);
    let repos = repos_for_search(c, m.hits.iter().map(|h| h.repo).collect(), who).await;
    let items: Vec<Value> = m
        .hits
        .iter()
        .filter_map(|h| {
            act::search_item(&repos.get(&h.repo)?.view(host), graftyard_git::issues::IssueNumber(h.number), h.score)
        })
        .collect();
    Ok(act::search_reply(&m, items))
}

async fn repos_for_search(c: &Ctx<'_>, ids: BTreeSet<RepoId>, who: Option<&Principal>) -> BTreeMap<RepoId, Read> {
    let reads = ids.into_iter().map(|id| async move {
        let repo = crate::repo_by_id(c, id).await.ok()?;
        let (owner, name, repo, a) = crate::api::repo_named(c, repo).await?;
        let rd = crate::api::read_repo(
            c,
            repo,
            &owner,
            &name,
            who,
            Some(a),
            crate::api::Rows::All,
            crate::api::Gate::Readers,
        )
        .await
        .ok()?
        .ok()?;
        Some((id, rd))
    });
    futures_util::future::join_all(reads).await.into_iter().flatten().collect()
}

/// One shard's answer within `SHARD_DEADLINE`, or `None` (left out, `incomplete_results`).
async fn ask(c: &Ctx<'_>, k: &SearchShard, body: &str) -> search::ShardAnswer {
    let call = async {
        let site = k.site(c.sites, &[])?;
        let j = k.domain.jurisdiction().map(|j| match j {
            graftyard_types::Jurisdiction::Eu => "eu",
            graftyard_types::Jurisdiction::Us => "us",
        });
        let stub = c.bindings().object("SEARCH", site, &k.do_name(), j).ok()?;
        let mut init = RequestInit::new();
        init.with_method(Method::Post).with_body(Some(body.to_owned().into()));
        let mut r = stub.fetch_with_request(Request::new_with_init("https://search/query", &init).ok()?).await.ok()?;
        if r.status_code() != 200 {
            return None;
        }
        let v: Value = r.json().await.ok()?;
        let hits = v["hits"]
            .as_array()?
            .iter()
            .filter_map(|h| {
                Some(search::Hit {
                    repo: RepoId(graftyard_types::DbId(h[0].as_i64()?)),
                    number: h[1].as_u64()? as u32,
                    updated: graftyard_types::Timestamp(h[2].as_u64()?),
                    score: h[3].as_f64()?,
                })
            })
            .collect();
        Some((hits, v["total"].as_u64().unwrap_or(0)))
    };
    graftyard_lookup::within(search::SHARD_DEADLINE, call).await.flatten()
}

/// The newest generation of each (domain, shard), from every site's `search_map` that answers.
async fn shard_map(c: &Ctx<'_>) -> BTreeMap<(Domain, u8), u32> {
    let reads = c.sites.0.iter().filter(|s| !s.spare).map(|s| async move {
        let stub = c.bindings().registry(s.id, "search_map").ok()?;
        let mut r = stub.fetch_with_str("https://registry/search_map").await.ok()?;
        r.json::<BTreeMap<String, u32>>().await.ok()
    });
    let mut out = BTreeMap::new();
    for m in futures_util::future::join_all(reads).await.into_iter().flatten() {
        for (k, g) in m {
            let Some(s) = SearchShard::parse(&format!("search#{k}#{g}")) else { continue };
            let e = out.entry((s.domain, s.shard)).or_insert(g);
            *e = (*e).max(g);
        }
    }
    out
}
