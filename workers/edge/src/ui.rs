//! The web app on the edge (`design/SHARED.md`, "Frontend and native app", "How the edge serves it"): the private
//! `/_t/ui/*` API, the web session cookie, the HTML shell with the first route's reads inlined, and the static assets.
//! Nothing here answers a GHES 3.22 path: `/_t` is reserved, the shell answers only a browser's `GET` of a path
//! nothing else claims, and the cookie counts only on a request that also carries `x-graftyard-web: 1`.

use graftyard_identity::{Actor, Credential, Principal, ScopeSet, UserId, WebSessionId};
use graftyard_types::{AccountId, ChainId, DbId, Ulid};
use graftyard_wire::{Host, Reply};
use serde_json::{Value, json};
use worker::*;

use crate::Ctx;
use crate::api::{Read, read, read_failed, unavailable};

pub const PREFIX: &str = "/_t/ui/";
const COOKIE: &str = "__Host-gy_session";
/// The cookie's lifetime [guess, GitHub's `user_session` lasts about two weeks].
const SESSION_DAYS: u64 = 14;
/// The header a cookie-authenticated request must carry, which a cross-site form cannot send without a CORS preflight
/// the edge never grants.
const WEB_HEADER: &str = "x-graftyard-web";
const DOC: &str = "graftyard/web";

/// The persisted GraphQL documents the app sends by hash (`web/src/api/persisted.json`, checked by `npm run check`).
const PERSISTED: &str = include_str!("../../../web/src/api/persisted.json");

// ---- the session ----

/// The caller a web session cookie names, when the request is the app's own (`web_request`) and the account's
/// `Identity` fold lists the session and its `Account` row was not signed out after it. `None` otherwise, as no
/// credential is.
pub async fn session(c: &Ctx<'_>, req: &Request) -> Result<Option<Principal>> {
    let h = req.headers();
    if !web_request(
        h.get(WEB_HEADER)?.as_deref(),
        h.get("upgrade")?.as_deref(),
        h.get("origin")?.as_deref(),
        &c.cfg.host,
    ) {
        return Ok(None);
    }
    cookie_session(c, req).await
}

/// Whether a request may use the session cookie: it carries `x-graftyard-web: 1`, or it is a WebSocket upgrade from
/// the edge's own origin. A browser's `WebSocket` cannot set a header, but it always sends `Origin`, which a page on
/// another origin cannot forge, so the live-log viewer's socket is checked by origin instead.
fn web_request(header: Option<&str>, upgrade: Option<&str>, origin: Option<&str>, host: &str) -> bool {
    header == Some("1")
        || (upgrade.is_some_and(|u| u.eq_ignore_ascii_case("websocket"))
            && origin.and_then(|o| o.strip_prefix("https://")) == Some(host))
}

/// The cookie's caller without the header check, for the shell's own document `GET` only: the seed it renders is
/// read-only and lands in HTML a cross-origin page cannot read, and `SameSite=Strict` keeps the cookie off a
/// cross-site navigation.
async fn cookie_session(c: &Ctx<'_>, req: &Request) -> Result<Option<Principal>> {
    let Some((account, id)) = cookie(req)? else { return Ok(None) };
    let Ok(Some(rec)) = crate::placement(c, account).await else { return Ok(None) };
    let fold = graftyard_lookup::read_spread(
        &c.bindings(),
        ChainId::Identity(account),
        &rec,
        &json!({ "q": { "dep": true } }),
    )
    .await;
    let Ok(Some((v, _, _))) = fold else { return Ok(None) };
    let Some(f) = v["value"]
        .as_str()
        .and_then(graftyard_lookup::unhex)
        .and_then(|b| graftyard_raft::codec::from_bytes::<graftyard_identity::IdentityFold>(&b).ok())
    else {
        return Ok(None);
    };
    if !f.sessions.contains_key(&id) {
        return Ok(None);
    }
    let row = crate::system_read(
        c,
        ChainId::Names(graftyard_identity::account_shard(account)),
        json!({ "account": account.0.0 }),
    )
    .await
    .map_err(Error::RustError)?;
    let row: Option<graftyard_identity::AccountRow> = serde_json::from_value(row["account"].clone()).unwrap_or(None);
    match row {
        Some(r) if !r.suspended && r.sessions_not_before <= id.0.time() => Ok(Some(Principal {
            actor: Actor::User(UserId(account)),
            scopes: ScopeSet(1),
            credential: Credential::Session(id),
            sso_orgs: smallvec::SmallVec::new(),
        })),
        _ => Ok(None),
    }
}

/// The cookie's account and session: `<account>.<session ulid>`.
fn cookie(req: &Request) -> Result<Option<(AccountId, WebSessionId)>> {
    let Some(h) = req.headers().get("cookie")? else { return Ok(None) };
    let v = h.split(';').filter_map(|kv| kv.trim().split_once('=')).find(|(k, _)| *k == COOKIE).map(|(_, v)| v);
    Ok(v.and_then(|v| {
        let (a, s) = v.split_once('.')?;
        Some((AccountId(DbId(a.parse().ok()?)), WebSessionId(s.parse::<Ulid>().ok()?)))
    }))
}

/// `POST /_t/ui/login/session` with `{login, password}`: checks the password against the account's `Identity` fold,
/// commits `SignIn { method: Password }`, and sets the cookie. 401 for any wrong login or password.
async fn sign_in(c: &Ctx<'_>, req: &mut Request) -> Result<Response> {
    let b: Value = req.json().await.unwrap_or(Value::Null);
    let (Some(login), Some(password)) = (b["login"].as_str(), b["password"].as_str()) else {
        return respond(c, Reply::error(422, "login and password", DOC));
    };
    let wrong = || Reply::error(401, "Incorrect username or password.", DOC);
    let Some((account, graftyard_identity::AccountKind::User)) =
        crate::ident::account_of(c, login.trim()).await.map_err(Error::RustError)?
    else {
        return respond(c, wrong());
    };
    let hash = match identity_fold(c, account).await {
        Ok(Some(f)) => f.password.map(|p| p.0),
        Ok(None) => None,
        Err(e) => return respond(c, unavailable(&e)),
    };
    if !hash.is_some_and(|h| graftyard_identity::password::verify_password(password, &h)) {
        return respond(c, wrong());
    }
    let id = WebSessionId(Ulid::new(crate::now(), crate::rand10()));
    let install = graftyard_identity::install_account();
    let graftyard_identity::finish::Step::Identity(chain, cmd) =
        graftyard_identity::edge::sign_in(account, id, graftyard_identity::SignInMethod::Password, install)
    else {
        unreachable!("an Identity step")
    };
    let r = match crate::ident::commit_owner(c, chain, account, serde_json::to_value(&cmd)?).await {
        Ok(d) => match crate::ident::refused(&d).as_deref() {
            None => {
                let login = crate::ident::login(c, account).await.unwrap_or_else(|| login.to_owned());
                let set = format!(
                    "{COOKIE}={}.{}; Path=/; Secure; HttpOnly; SameSite=Strict; Max-Age={}",
                    account.0.0,
                    id.0,
                    SESSION_DAYS * 86_400
                );
                Reply::json(200, json!({ "login": login, "two_factor": false, "attempt": "" })).with("set-cookie", set)
            }
            Some("WrongMethod") => Reply::error(403, "This install does not sign in with a password.", DOC),
            Some("Suspended") => wrong(),
            Some(e) => unavailable(&format!("the sign-in did not commit: {e}")),
        },
        Err(x) => x,
    };
    respond(c, r)
}

/// `PUT /_t/ui/settings/password` with `{password}`, under the account's own credential: `SetPassword` with a new
/// PBKDF2 hash. 204 after its quorum.
async fn set_password(c: &Ctx<'_>, req: &mut Request) -> Result<Response> {
    let Some(who) = crate::authenticate(c, req).await? else {
        return respond(c, Reply::error(401, "Requires authentication", DOC));
    };
    let Actor::User(me) = who.actor else { return respond(c, Reply::error(403, "Only a user has a password", DOC)) };
    let b: Value = req.json().await.unwrap_or(Value::Null);
    let Some(p) = b["password"].as_str().filter(|p| p.chars().count() >= graftyard_identity::password::PASSWORD_MIN)
    else {
        return respond(c, Reply::error(422, "password must be at least 8 characters", DOC));
    };
    let mut salt = [0u8; 16];
    salt[..10].copy_from_slice(&crate::rand10());
    salt[10..].copy_from_slice(&crate::rand10()[..6]);
    let hash = graftyard_identity::password::hash_password(p, &salt);
    let cmd = json!({ "cred": who.credential_shard(), "op": { "SetPassword": { "hash": hash } } });
    let r = match crate::ident::commit_owner(c, ChainId::Identity(me.0), me.0, cmd).await {
        Ok(d) => match crate::ident::refused(&d).as_deref() {
            None => Reply::empty(204),
            Some("Unauthorized") => Reply::error(401, "Bad credentials", DOC),
            Some(e) => unavailable(&format!("the password did not commit: {e}")),
        },
        Err(x) => x,
    };
    respond(c, r)
}

/// The account's whole `Identity` fold, read through `fresh` on its group.
async fn identity_fold(
    c: &Ctx<'_>,
    account: AccountId,
) -> std::result::Result<Option<graftyard_identity::IdentityFold>, String> {
    let Some(rec) = crate::placement(c, account).await? else { return Ok(None) };
    let body = json!({ "q": { "dep": true } }).to_string();
    let Some((v, _, _)) =
        graftyard_lookup::call_opt(&c.bindings(), ChainId::Identity(account), &rec, "/read", body.as_bytes()).await?
    else {
        return Ok(None);
    };
    let bytes = v["value"].as_str().and_then(graftyard_lookup::unhex).ok_or("identity dep: not hex")?;
    graftyard_raft::codec::from_bytes(&bytes).map(Some).map_err(|e| e.to_string())
}

// ---- the private API ----

fn respond(c: &Ctx<'_>, r: Reply) -> Result<Response> {
    let mut r = r;
    if !r.headers.iter().any(|(k, _)| *k == "content-type") {
        r = r.with("content-type", "application/json; charset=utf-8");
    }
    crate::agents::respond(c, r.with("cache-control", "private, no-store"))
}

/// One request under `/_t/ui/`.
pub async fn serve(c: &Ctx<'_>, req: &mut Request, host: &Host, rest: &str) -> Result<Response> {
    let segs: Vec<&str> = rest.split('/').collect();
    let method = req.method();
    match (&method, segs.as_slice()) {
        (Method::Post, ["login", "session"]) => return sign_in(c, req).await,
        (Method::Put, ["settings", "password"]) => return set_password(c, req).await,
        (Method::Get, ["repos", o, r, tail @ ..]) => {
            c.read_class.set(true);
            let who = crate::authenticate(c, req).await?;
            let q = req.url()?.query().unwrap_or("").to_owned();
            let reply = repo_get(c, host, o, r, tail, &q, who.as_ref()).await?;
            respond(c, reply)
        }
        _ => respond(c, Reply::not_found()),
    }
}

/// A repository read under `/_t/ui/repos/{o}/{r}/`.
async fn repo_get(
    c: &Ctx<'_>,
    host: &Host,
    o: &str,
    r: &str,
    tail: &[&str],
    query: &str,
    who: Option<&Principal>,
) -> Result<Reply> {
    if !matches!(
        tail,
        ["resolve", ..]
            | ["tree-commits", _, ..]
            | ["blame", _, _, ..]
            | ["readme"]
            | ["pulls", _, "files" | "commits" | "events" | "threads" | "merge-requirements" | "viewed"]
            | ["subscription", "events"]
    ) {
        return Ok(Reply::not_found());
    }
    let rd = match read(c, o, r, who).await? {
        Ok(rd) => rd,
        Err(x) => return Ok(x),
    };
    let v = rd.view(host);
    Ok(match tail {
        ["subscription", "events"] => Reply::json(200, json!({ "events": null })),
        ["pulls", n, "viewed"] => match graftyard_api::ui::pull_shas(&v, n) {
            Some(_) => Reply::json(200, json!({ "paths": [] })),
            None => Reply::not_found(),
        },
        ["pulls", n, "events"] => graftyard_api::ui::events(&v, n),
        ["pulls", n, "threads"] => graftyard_api::ui::threads(&v, n),
        ["pulls", n, "merge-requirements"] => {
            let Some((base, head)) = graftyard_api::ui::pull_shas(&v, n) else { return Ok(Reply::not_found()) };
            let behind = match shard_json(c, &rd, "compare", &[("base", base.hex()), ("head", head.hex())]).await {
                Ok(x) => x["behind_by"].as_u64().unwrap_or(0) as u32,
                Err(x) => return Ok(x),
            };
            graftyard_api::ui::merge_requirements(&v, n, behind)
        }
        ["pulls", n, what @ ("files" | "commits")] => {
            let Some((base, head)) = graftyard_api::ui::pull_shas(&v, n) else { return Ok(Reply::not_found()) };
            let route = if *what == "files" { "diff" } else { "commits" };
            match shard_json(c, &rd, route, &[("base", base.hex()), ("head", head.hex())]).await {
                Ok(x) if *what == "files" => graftyard_api::ui::files(&v, head, &x),
                Ok(x) => graftyard_api::ui::commits(&v, &x),
                Err(x) => x,
            }
        }
        ["resolve", splat @ ..] => {
            let splat = splat.join("/");
            match shard_json(c, &rd, "resolve", &[("splat", decode(&splat))]).await {
                Ok(x) => Reply::json(200, x),
                Err(x) => x,
            }
        }
        ["tree-commits", reference, path @ ..] | ["blame", reference, path @ ..] => {
            let Some(sha) = graftyard_api::rest::resolve(&v, &decode(reference)) else { return Ok(Reply::not_found()) };
            let route = if tail[0] == "blame" { "blame" } else { "tree" };
            match shard_json(c, &rd, route, &[("commit", sha.hex()), ("path", decode(&path.join("/")))]).await {
                Ok(x) => Reply::json(200, x),
                Err(x) => x,
            }
        }
        ["readme"] => {
            let reference = url::form_urlencoded::parse(query.as_bytes())
                .find(|(k, _)| k == "ref")
                .map(|(_, v)| v.into_owned())
                .or_else(|| v.settings().map(|s| s.default_branch.display()));
            let Some(sha) = reference.and_then(|x| graftyard_api::rest::resolve(&v, &x)) else {
                return Ok(Reply::not_found());
            };
            match shard_json(c, &rd, "readme", &[("commit", sha.hex()), ("path", String::new())]).await {
                Ok(x) => Reply::json(200, x),
                Err(x) => x,
            }
        }
        _ => Reply::not_found(),
    })
}

fn decode(s: &str) -> String {
    url::form_urlencoded::parse(format!("x={}", s.replace('+', "%2B")).as_bytes())
        .next()
        .map(|(_, v)| v.into_owned())
        .unwrap_or_default()
}

/// Reads the git cache's counts for every (base tip, head) pair a render of `rd` asked for (`HostedDir::asked`) into
/// `rd.dir.compares`, so the next render shows GHES's `commits`, `additions`, `deletions`, and `changed_files`. Returns
/// whether it read any. A pair the shard did not answer stays 0, as before the shard computed counts.
pub(crate) async fn read_compares(c: &Ctx<'_>, rd: &Read) -> bool {
    let mut asked = std::mem::take(&mut *rd.dir.asked.borrow_mut());
    if asked.is_empty() {
        return false;
    }
    // A list that selects the counts reads at most `COMPARES_MAX` pairs; the rest count 0.
    asked.truncate(COMPARES_MAX);
    let reads = asked
        .iter()
        .map(|(b, h)| async move { shard_json(c, rd, "compare", &[("base", b.hex()), ("head", h.hex())]).await });
    let got = futures_util::future::join_all(reads).await;
    for (pair, x) in asked.into_iter().zip(got) {
        record_compare(rd, pair, x.as_ref().unwrap_or(&Value::Null));
    }
    true
}

/// The most (base tip, head) pairs one request reads from the shard for counts [guess: a page of 25 pull requests,
/// with room, well under the Workers subrequest limit].
const COMPARES_MAX: usize = 50;

/// Records the shard's `/ui/compare` reply for `pair` as the counts the next render of `rd` shows.
fn record_compare(rd: &Read, pair: (graftyard_git::refs::ObjectId, graftyard_git::refs::ObjectId), x: &Value) {
    let n = |k: &str| x[k].as_u64().map_or(0, |v| v as u32);
    let counts = graftyard_api::view::Compare {
        commits: n("ahead_by"),
        additions: n("additions"),
        deletions: n("deletions"),
        changed_files: n("changed_files"),
    };
    rd.dir.compares.borrow_mut().push((pair, counts));
}

/// One `/ui/<route>` read on the network's git cache shards (`crate::shard_read`): the shard's JSON, its 4xx as is,
/// or 503 with `Retry-After` for anything else.
pub(crate) async fn shard_json(
    c: &Ctx<'_>,
    rd: &Read,
    route: &str,
    q: &[(&str, String)],
) -> std::result::Result<Value, Reply> {
    let mut s = url::form_urlencoded::Serializer::new(String::new());
    s.append_pair("repo", &rd.repo.id.0.0.to_string());
    for (k, v) in q {
        s.append_pair(k, v);
    }
    let url = format!("https://shard/ui/{route}?{}", s.finish());
    let mut r = match crate::shard_read(c, &rd.repo, &url, Method::Get, "version=2", None).await {
        Ok(r) => r,
        Err(e) => return Err(read_failed("the git cache did not answer; retry", &e.to_string())),
    };
    let status = r.status_code();
    let text = r.text().await.unwrap_or_default();
    match status {
        200 => serde_json::from_str(&text).map_err(|e| unavailable(&format!("the git cache's reply: {e}"))),
        404 => Err(Reply::not_found()),
        400 | 422 => Err(Reply::error(422, text.trim(), DOC)),
        _ => Err(read_failed("the git cache did not answer; retry", &format!("{status} {}", text.trim()))),
    }
}

// ---- persisted GraphQL ----

/// The text of a persisted document named by `extensions.persistedQuery.sha256Hash`, for a request with no `query`.
/// `Ok(None)`: the request carries its own text, or no persisted query at all. `Err`: GraphQL's
/// `PersistedQueryNotFound` at 200.
pub fn persisted(body: &Value) -> std::result::Result<Option<String>, Reply> {
    if body.get("query").and_then(Value::as_str).is_some() {
        return Ok(None);
    }
    let Some(hash) = body.pointer("/extensions/persistedQuery/sha256Hash").and_then(Value::as_str) else {
        return Ok(None);
    };
    let docs: Value = serde_json::from_str(PERSISTED).unwrap_or(Value::Null);
    let found = docs
        .as_object()
        .into_iter()
        .flatten()
        .find(|(_, d)| d["sha256"].as_str() == Some(hash))
        .and_then(|(_, d)| d["query"].as_str().map(str::to_owned));
    found.map(Some).ok_or_else(|| {
        Reply::json(
            200,
            json!({ "errors": [{ "message": "PersistedQueryNotFound",
                "extensions": { "code": "PERSISTED_QUERY_NOT_FOUND" } }] }),
        )
    })
}

// ---- the shell and assets ----

/// Whether this request is the web app's: a `GET` of `/assets/*`, or a browser's `GET` (accepting `text/html`) of a
/// path nothing else serves, which the caller decides.
pub fn wants_shell(req: &Request) -> Result<bool> {
    Ok(req.method() == Method::Get && req.headers().get("accept")?.is_some_and(|a| a.contains("text/html")))
}

/// `/assets/*` (immutable) and `/favicon.ico` from the `ASSETS` binding; `None` when the build has no such file.
pub async fn asset(c: &Ctx<'_>, req: Request) -> Result<Option<Response>> {
    let Ok(assets) = c.env.assets("ASSETS") else { return Ok(None) };
    let hashed = req.path().starts_with("/assets/");
    let r = assets.fetch_request(req).await?;
    if r.status_code() != 200 {
        return Ok(None);
    }
    let h = r.headers().clone();
    // A content-hashed file never changes; the favicon has no hash.
    let immutable = hashed;
    h.set("cache-control", if immutable { "public, max-age=31536000, immutable" } else { "public, max-age=3600" })?;
    Ok(Some(r.with_headers(h)))
}

/// The HTML shell, with the first route's reads inlined when the path is a repository's overview or a pull request.
pub async fn shell(c: &Ctx<'_>, req: &Request, host: &Host, path: &str) -> Result<Response> {
    let Ok(assets) = c.env.assets("ASSETS") else { return crate::text(c.rid, 404, "Not Found\n") };
    let url = req.url()?;
    let mut shell_url = url.clone();
    shell_url.set_path("/_shell.html");
    shell_url.set_query(None);
    let mut r = assets.fetch(shell_url.to_string(), None).await?;
    if r.status_code() != 200 {
        return crate::text(c.rid, 503, "the web app's shell is missing from this build\n");
    }
    let html = r.text().await?;
    let tab = url.query_pairs().find(|(k, _)| k == "tab").map(|(_, v)| v.into_owned());
    let seed = seed(c, req, host, path, tab.as_deref()).await;
    let html = match seed {
        Some(s) => inline(&html, &s),
        None => html,
    };
    let h = crate::headers(c.rid)?;
    h.set("content-type", "text/html; charset=utf-8")?;
    h.set("cache-control", "private, no-store")?;
    Ok(Response::ok(html)?.with_headers(h))
}

/// `html` with the seed script before `</head>`. `<` is escaped, so no string in the data can close the script.
fn inline(html: &str, seed: &Value) -> String {
    let data = seed.to_string().replace('<', "\\u003c");
    let tag = format!("<script id=\"__graft_seed\" type=\"application/json\">{data}</script>");
    match html.find("</head>") {
        Some(i) => format!("{}{tag}{}", &html[..i], &html[i..]),
        None => format!("{tag}{html}"),
    }
}

/// The reads the first route's regions make on first render, rendered here with the renderers that serve them,
/// under the query keys the app's `queryOptions` use. A read that fails is left out, so its region fetches it.
async fn seed(c: &Ctx<'_>, req: &Request, host: &Host, path: &str, tab: Option<&str>) -> Option<Value> {
    let segs: Vec<&str> = path.trim_matches('/').split('/').collect();
    let (o, r, pull) = match segs.as_slice() {
        [o, r] => (*o, *r, None),
        [o, r, "pull", n] => (*o, *r, Some(*n)),
        _ => return None,
    };
    c.read_class.set(true);
    let who = match crate::authenticate(c, req).await.ok().flatten() {
        Some(p) => Some(p),
        None => cookie_session(c, req).await.ok().flatten(),
    };
    let mut out: Vec<Value> = Vec::new();
    if let Some(Actor::User(me)) = who.as_ref().map(|w| w.actor)
        && let Some(login) = crate::ident::login(c, me.0).await
    {
        out.push(json!([["user"], crate::ident::user_json(c, me.0, &login, false)]));
    }
    let rd = read(c, o, r, who.as_ref()).await.ok()?.ok()?;
    let v = rd.view(host);
    let ok = |x: Reply| (x.status == 200).then(|| x.value().clone());
    let key = |rest: &[Value]| {
        let mut k = vec![json!("repos"), json!(o), json!(r)];
        k.extend_from_slice(rest);
        Value::Array(k)
    };
    if let Some(repo) = ok(graftyard_api::repos::get(&v, graftyard_types::Index(rd.index))) {
        out.push(json!([key(&[]), repo]));
    }
    match pull {
        None => {
            let branch = v.settings().map(|s| s.default_branch.display());
            let branch = branch.as_deref().map(|b| b.strip_prefix("refs/heads/").unwrap_or(b).to_owned());
            if let Some(b) = branch
                && let Some(sha) = graftyard_api::rest::resolve(&v, &b)
            {
                let q = [("commit", sha.hex()), ("path", String::new())];
                let tree = shard_json(c, &rd, "tree", &q);
                let readme = shard_json(c, &rd, "readme", &q);
                let (tree, readme) = futures_util::future::join(tree, readme).await;
                if let Ok(t) = tree {
                    out.push(json!([["_t", "repos", o, r, "tree-commits", b, ""], t]));
                }
                if let Ok(x) = readme {
                    out.push(json!([["_t", "repos", o, r, "readme", null], x]));
                }
            }
        }
        Some(n) => {
            let num: u32 = n.parse().ok()?;
            let nk = |rest: &[&str]| {
                let mut k = vec![json!("pulls"), json!(num)];
                k.extend(rest.iter().map(|s| json!(s)));
                key(&k)
            };
            for (k, x) in [
                ("reviews", graftyard_api::rest::list_reviews(&v, n, "per_page=100")),
                ("comments", graftyard_api::rest::list_review_comments(&v, n, "per_page=100")),
                ("threads", graftyard_api::ui::threads(&v, n)),
            ] {
                if let Some(x) = ok(x) {
                    out.push(json!([nk(&[k]), x]));
                }
            }
            let shas = graftyard_api::ui::pull_shas(&v, n);
            let compare = async {
                match shas {
                    Some((b, h)) => shard_json(c, &rd, "compare", &[("base", b.hex()), ("head", h.hex())]).await.ok(),
                    None => None,
                }
            };
            let commits = async {
                match shas {
                    Some((b, h)) => shard_json(c, &rd, "commits", &[("base", b.hex()), ("head", h.hex())]).await.ok(),
                    None => None,
                }
            };
            let files = async {
                match (shas, tab) {
                    (Some((b, h)), Some("files")) => {
                        shard_json(c, &rd, "diff", &[("base", b.hex()), ("head", h.hex())]).await.ok()
                    }
                    _ => None,
                }
            };
            let (compare, commits, files) = futures_util::future::join3(compare, commits, files).await;
            // The pull renders after the compare, so its counts are the shard's.
            if let (Some(pair), Some(x)) = (shas, &compare) {
                record_compare(&rd, pair, x);
            }
            let (pr, _) = graftyard_api::rest::get_pull_asks(&v, n);
            let Some(pr) = ok(pr) else { return Some(json!({ "at": js_sys::Date::now() as u64, "queries": out })) };
            let head = pr["head"]["sha"].as_str().unwrap_or_default().to_owned();
            out.push(json!([nk(&[]), pr]));
            if let Some(x) = compare
                && let Some(m) =
                    ok(graftyard_api::ui::merge_requirements(&v, n, x["behind_by"].as_u64().unwrap_or(0) as u32))
            {
                out.push(json!([nk(&["merge-requirements"]), m]));
            }
            if let Some(x) = commits
                && let Some(x) = ok(graftyard_api::ui::commits(&v, &x))
            {
                out.push(json!([nk(&["commits"]), x]));
            }
            match tab {
                None => {
                    for (k, x) in [
                        ("events", graftyard_api::ui::events(&v, n)),
                        ("comments", graftyard_api::rest::list_issue_comments(&v, n, "per_page=100")),
                    ] {
                        if let Some(x) = ok(x) {
                            let k = if k == "comments" {
                                key(&[json!("issues"), json!(num), json!("comments")])
                            } else {
                                nk(&[k])
                            };
                            out.push(json!([k, x]));
                        }
                    }
                }
                Some("files") => {
                    if let Some((_, h)) = shas
                        && let Some(x) = files
                        && let Some(x) = ok(graftyard_api::ui::files(&v, h, &x))
                    {
                        out.push(json!([nk(&["files"]), x]));
                    }
                    out.push(json!([nk(&["viewed"]), { "paths": [] }]));
                }
                _ => {}
            }
            if !head.is_empty() {
                let sha = json!(head);
                if let Some(x) = ok(graftyard_api::rest::list_check_runs_for_ref(&v, &head, "")) {
                    out.push(json!([key(&[json!("commits"), sha.clone(), json!("check-runs")]), x]));
                }
                if let Some(x) = ok(graftyard_api::rest::list_commit_statuses(
                    &v,
                    &head,
                    graftyard_wire::Page::parse("per_page=100"),
                )) {
                    out.push(json!([key(&[json!("commits"), sha, json!("statuses")]), x]));
                }
            }
        }
    }
    Some(json!({ "at": js_sys::Date::now() as u64, "queries": out }))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    /// A string in the data cannot end the script early: every `<` is escaped.
    #[test]
    fn the_seed_cannot_close_its_script() {
        let html = "<html><head><title>x</title></head><body></body></html>";
        let out = super::inline(html, &json!({ "q": "</script><script>alert(1)</script>" }));
        assert_eq!(out.matches("</script>").count(), 1, "{out}");
        assert!(out.contains("<script id=\"__graft_seed\" type=\"application/json\">"));
        assert!(out.find("__graft_seed").unwrap() < out.find("</head>").unwrap());
    }

    /// The cookie counts for the app's fetches and for a WebSocket from the edge's own origin, and never for a socket
    /// another origin opens, a plain request, or a lookalike host.
    #[test]
    fn the_cookie_counts_only_for_the_apps_own_requests() {
        let host = "graft.example";
        let ws = Some("websocket");
        assert!(super::web_request(Some("1"), None, None, host));
        assert!(super::web_request(None, Some("WebSocket"), Some("https://graft.example"), host));
        assert!(!super::web_request(None, ws, Some("https://evil.example"), host));
        assert!(!super::web_request(None, ws, Some("https://graft.example.evil.example"), host));
        assert!(!super::web_request(None, ws, Some("http://graft.example"), host));
        assert!(!super::web_request(None, ws, None, host));
        assert!(!super::web_request(None, None, Some("https://graft.example"), host));
        assert!(!super::web_request(Some("0"), None, Some("https://graft.example"), host));
    }

    /// A persisted request is looked up by hash; one with its own text passes through; an unknown hash is GraphQL's
    /// `PersistedQueryNotFound`.
    #[test]
    fn persisted_documents_are_found_by_hash() {
        let docs: serde_json::Value = serde_json::from_str(super::PERSISTED).unwrap();
        let (_, d) = docs.as_object().unwrap().iter().next().unwrap();
        let hash = d["sha256"].as_str().unwrap();
        let ask = |h: &str| json!({ "operationName": "X", "extensions": { "persistedQuery": { "version": 1, "sha256Hash": h } } });
        assert_eq!(super::persisted(&ask(hash)).unwrap().as_deref(), d["query"].as_str());
        assert_eq!(super::persisted(&json!({ "query": "{ viewer { login } }" })).unwrap(), None);
        let miss = super::persisted(&ask(&"0".repeat(64))).unwrap_err();
        assert_eq!(miss.value()["errors"][0]["message"], "PersistedQueryNotFound");
    }
}
