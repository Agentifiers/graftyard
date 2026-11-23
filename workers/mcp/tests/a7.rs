//! Journey A7 through `graft-mcp`'s routes (`graftyard_mcp::flow`), with the host's grant endpoints and `token/scoped`
//! over real members and the edge's OAuth stood in (`common/bed.rs`), and L0-08's checks that run locally
//! (`design/AGENT-LAYER.md`, Build order, the MCP row; `build/BUILD-PLAN.md`, AM2-WP1, LM-2's MCP row):
//!
//! - 100 of 100 wrong-audience tokens refused;
//! - a refreshed token keeps its scope, 100 of 100 chained refreshes, and no refresh is wider than its grant;
//! - a replayed refresh revokes the grant;
//! - framed, cross-site, or skipped consent issues no code;
//! - a refresh succeeds with the unscoped `ghu_` held, which is deleted after it.
//!
//! What this bed cannot show: a browser engine (framing is checked by header) and real SDK clients, which L0-08 hosted
//! measures.

#[path = "common/bed.rs"]
mod bed;
#[path = "../../../crates/identity/tests/common/mod.rs"]
mod common;

use std::collections::BTreeMap;

use bed::{Bed, CIMD, ORIGIN, REDIRECT};
use graftyard_identity::{RepoScope, TokenKind, WriteScope};
use graftyard_mcp::flow::{FLOW_COOKIE, Req, Resp};
use graftyard_mcp::{McpRefresh, McpToken, Scope, Sealed, Sealer, Toolset, ToolsetMask, Url, s256};
use serde_json::Value;

const VERIFIER: &str = "dBjftJeZ4CVP-mJ92K9-tR-u4jXkL0l_WjyXc4fnd1s";

fn get(path: &str) -> Req {
    Req {
        method: "GET".into(),
        path: path.split('?').next().unwrap().into(),
        query: path.split_once('?').map(|x| x.1).unwrap_or("").into(),
        ..Req::default()
    }
}

fn post(path: &str, form: &[(&str, &str)], headers: &[(&str, &str)]) -> Req {
    let body = url::form_urlencoded::Serializer::new(String::new()).extend_pairs(form).finish().into_bytes();
    Req {
        method: "POST".into(),
        path: path.into(),
        headers: headers.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect(),
        body,
        ..Req::default()
    }
}

fn query(location: &str) -> BTreeMap<String, String> {
    url::Url::parse(location).unwrap().query_pairs().into_owned().collect()
}

fn json(r: &Resp) -> Value {
    serde_json::from_slice(&r.body).unwrap_or(Value::Null)
}

fn html(r: &Resp) -> String {
    String::from_utf8_lossy(&r.body).into_owned()
}

fn authorize_path(scope: &str) -> String {
    let mut s = url::form_urlencoded::Serializer::new(String::new());
    s.append_pair("response_type", "code")
        .append_pair("client_id", CIMD)
        .append_pair("redirect_uri", REDIRECT)
        .append_pair("state", "client-state-1")
        .append_pair("code_challenge", &s256(VERIFIER))
        .append_pair("code_challenge_method", "S256")
        .append_pair("resource", &format!("{ORIGIN}/"))
        .append_pair("scope", scope);
    format!("/authorize?{}", s.finish())
}

/// The consent page's sealed form field.
fn pending(page: &Resp) -> String {
    let h = html(page);
    let at = h.find("name=p value=\"").expect("the form") + 14;
    h[at..at + h[at..].find('"').unwrap()].to_owned()
}

fn same_origin() -> Vec<(&'static str, &'static str)> {
    vec![("origin", ORIGIN), ("sec-fetch-site", "same-origin")]
}

/// Consent, the host's OAuth, and the callback, for user `i`: our code at the client's redirect.
fn code_for(b: &Bed, i: usize, scope: &str) -> String {
    b.browser.set(i);
    let page = b.serve(get(&authorize_path(scope)));
    assert_eq!(page.status, 200, "{}", html(&page));
    let r = b.serve(post(
        "/consent",
        &[("p", &pending(&page)), ("decision", "allow"), ("repos", "acme/mono")],
        &same_origin(),
    ));
    assert_eq!(r.status, 302, "{}", html(&r));
    let cookie = r.header("set-cookie").unwrap().split(';').next().unwrap().to_owned();
    assert!(cookie.starts_with(&format!("{FLOW_COOKIE}=")));
    let mut cb = get(&b.host_authorize(r.header("location").unwrap()));
    cb.headers.push(("cookie".into(), cookie));
    let r = b.serve(cb);
    assert_eq!(r.status, 302, "{}", html(&r));
    let q = query(r.header("location").unwrap());
    assert_eq!((q["state"].as_str(), q["iss"].as_str()), ("client-state-1", ORIGIN), "RFC 9207 iss on the answer");
    assert!(r.header("location").unwrap().starts_with(REDIRECT));
    q["code"].clone()
}

fn redeem(b: &Bed, code: &str) -> Resp {
    b.serve(post(
        "/token",
        &[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("client_id", CIMD),
            ("redirect_uri", REDIRECT),
            ("code_verifier", VERIFIER),
            ("resource", &format!("{ORIGIN}/")),
        ],
        &[],
    ))
}

fn refresh(b: &Bed, rt: &str, scope: Option<&str>) -> Resp {
    let mut f = vec![("grant_type", "refresh_token"), ("refresh_token", rt), ("client_id", CIMD)];
    if let Some(s) = scope {
        f.push(("scope", s));
    }
    b.serve(post("/token", &f, &[]))
}

fn sealer() -> Sealer {
    Sealer::new(vec![[0x5a; 32]]).unwrap()
}

/// A legacy-lane `ping` with `bearer`: 200 once past the door.
fn mcp_call(b: &Bed, bearer: &str) -> Resp {
    let mut r = post("/", &[], &[("content-type", "application/json")]);
    r.body = br#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#.to_vec();
    r.headers.push(("authorization".into(), format!("Bearer {bearer}")));
    b.serve(r)
}

#[test]
fn metadata_names_pkce_cimd_and_iss_and_lists_only_the_read_scope() {
    let b = Bed::new();
    let prm = json(&b.serve(get("/.well-known/oauth-protected-resource")));
    assert_eq!(prm["resource"], format!("{ORIGIN}/"));
    assert_eq!(prm["authorization_servers"][0], ORIGIN);
    assert_eq!(prm["scopes_supported"], serde_json::json!(["write:read"]));
    let asm = json(&b.serve(get("/.well-known/oauth-authorization-server")));
    assert_eq!(asm["issuer"], ORIGIN);
    assert_eq!(asm["code_challenge_methods_supported"], serde_json::json!(["S256"]));
    assert_eq!(
        (
            asm["client_id_metadata_document_supported"].as_bool(),
            asm["authorization_response_iss_parameter_supported"].as_bool()
        ),
        (Some(true), Some(true))
    );
    // No token: 401 naming the metadata.
    let r = b.serve(post("/", &[], &[]));
    assert_eq!(r.status, 401);
    assert_eq!(
        r.header("www-authenticate").unwrap(),
        format!("Bearer resource_metadata=\"{ORIGIN}/.well-known/oauth-protected-resource\"")
    );
}

#[test]
fn a7_end_to_end_issues_a_narrowed_token_and_a_grant_row() {
    let b = Bed::new();
    let code = code_for(&b, 0, "write:write tools:issues,pull_requests");
    // A wrong verifier, another client, or another redirect gets nothing and spends nothing.
    let wrong = |k: &str, v: &str| {
        let mut f = vec![
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("client_id", CIMD),
            ("redirect_uri", REDIRECT),
            ("code_verifier", VERIFIER),
        ];
        f.iter_mut().find(|(n, _)| *n == k).unwrap().1 = v;
        b.serve(post("/token", &f, &[]))
    };
    for (k, v) in [
        ("code_verifier", "x".repeat(43)),
        ("client_id", "https://client.example/other.json".into()),
        ("redirect_uri", "http://127.0.0.1:1/cb".into()),
    ] {
        let r = wrong(k, &v);
        assert_eq!((r.status, json(&r)["error"].as_str()), (400, Some("invalid_grant")), "{k}");
    }
    let g0 = b.identity_fold(b.user(0)).grants().values().next().unwrap().clone();
    assert_eq!((g0.grant.generation, g0.revoked), (0, false), "none of them spent the code");
    let r = redeem(&b, &code);
    assert_eq!(r.status, 200, "{}", html(&r));
    let v = json(&r);
    assert_eq!(v["scope"], "write:write tools:issues,pull_requests");
    let at = v["access_token"].as_str().unwrap();
    assert!(at.starts_with("gymcp_") && v["refresh_token"].as_str().unwrap().starts_with("gymcr_"));
    let t: McpToken = sealer().open(Sealed::Access, at).unwrap();
    assert_eq!(
        (t.aud.0.as_ref(), t.client.0.as_ref(), t.write),
        (format!("{ORIGIN}/").as_str(), CIMD, WriteScope::Write)
    );
    assert_eq!(t.repos, vec!["acme/mono".to_owned()]);
    // The upstream token is the host's narrowing: repository, owner, grant, and write scope on its row.
    let (row, f) = b.row(&t.upstream).unwrap();
    let f = f.unwrap();
    assert_eq!((row.kind, row.account), (TokenKind::UserToServer, b.user(0)));
    assert_eq!((f.resource_owner, f.write), (Some(b.owner), Some(WriteScope::Write)));
    assert_eq!(f.repos, Some(RepoScope::Selected(smallvec::smallvec![bed::REPO])));
    // The grant row records the consent, and its generation moved once, at redemption.
    let fold = b.identity_fold(b.user(0));
    let g = fold.grants().values().next().unwrap();
    assert_eq!((g.grant.client.0.as_ref(), g.grant.write, g.grant.generation), (CIMD, WriteScope::Write, 1));
    assert_eq!(g.grant.redirect_hosts.as_slice(), &["127.0.0.1".into()]);
    // The door takes it.
    assert_eq!(mcp_call(&b, at).status, 200);
    // A code redeems once: the second is a stale generation, so the grant is revoked with its tokens.
    let again = redeem(&b, &code);
    assert_eq!((again.status, json(&again)["error"].as_str()), (400, Some("invalid_grant")));
    assert!(b.identity_fold(b.user(0)).grants().values().next().unwrap().revoked);
    assert!(b.revoked(&t.upstream), "the host revoked the scoped token with the grant");
}

#[test]
fn l008_wrong_audience_tokens_are_refused_100_of_100() {
    let b = Bed::new();
    let code = code_for(&b, 0, "write:read");
    let good: McpToken =
        sealer().open(Sealed::Access, json(&redeem(&b, &code))["access_token"].as_str().unwrap()).unwrap();
    let other_key = Sealer::new(vec![[0x11; 32]]).unwrap();
    let mut refused = 0;
    for i in 0..100u8 {
        let aud = [
            "https://git.acme.test/",
            "https://mcp.other.test/",
            "https://mcp.git.acme.test.evil/",
            "https://mcp.git.acme.test",
        ][usize::from(i) % 4];
        let t = McpToken { aud: Url(aud.into()), ..good.clone() };
        let r = mcp_call(&b, &sealer().seal(Sealed::Access, &t, [i; 12]));
        assert_eq!(r.status, 401, "{aud}");
        assert!(r.header("www-authenticate").unwrap().contains("resource_metadata="));
        refused += 1;
        // Extras, as M0-WP15 ran them: our audience under another key, a refresh token, the upstream host token, expired.
        assert_eq!(mcp_call(&b, &other_key.seal(Sealed::Access, &good, [i; 12])).status, 401);
        assert_eq!(mcp_call(&b, &sealer().seal(Sealed::Refresh, &good, [i; 12])).status, 401);
        assert_eq!(mcp_call(&b, &good.upstream).status, 401);
        let old = McpToken { exp: b.now.get(), ..good.clone() };
        assert_eq!(mcp_call(&b, &sealer().seal(Sealed::Access, &old, [i; 12])).status, 401);
    }
    assert_eq!(refused, 100);
    assert_eq!(mcp_call(&b, &sealer().seal(Sealed::Access, &good, [0; 12])).status, 200, "the control passes the door");
}

#[test]
fn l008_a_refreshed_token_keeps_its_scope_and_the_held_ghu_goes_after_the_refresh() {
    let b = Bed::new();
    let code = code_for(&b, 0, "write:write tools:issues,checks");
    let v = json(&redeem(&b, &code));
    let want = Scope {
        write: WriteScope::Write,
        tools: ToolsetMask(ToolsetMask::of(Toolset::Issues).0 | ToolsetMask::of(Toolset::Checks).0),
    };
    let mut rt = v["refresh_token"].as_str().unwrap().to_owned();
    let mut kept = 0;
    for i in 0..100 {
        let before: McpRefresh = sealer().open(Sealed::Refresh, &rt).unwrap();
        assert!(!b.revoked(&before.held), "round {i}: the unscoped ghu_ is held, live, until this refresh");
        assert!(!b.deleted.borrow().contains(&before.held));
        let r = refresh(&b, &rt, None);
        assert_eq!(r.status, 200, "round {i}: {}", html(&r));
        let v = json(&r);
        assert_eq!(v["scope"], want.wire(), "round {i}");
        let t: McpToken = sealer().open(Sealed::Access, v["access_token"].as_str().unwrap()).unwrap();
        let next: McpRefresh = sealer().open(Sealed::Refresh, v["refresh_token"].as_str().unwrap()).unwrap();
        assert_eq!((t.write, t.tools, &t.repos), (want.write, want.tools, &before.repos));
        assert_eq!(next.generation, before.generation + 1);
        // Each refresh called token/scoped again: a new upstream token, carrying the grant's write scope.
        let (_, f) = b.row(&t.upstream).unwrap();
        assert_eq!(f.unwrap().write, Some(WriteScope::Write));
        // The old ghr_ was spent, and the old held ghu_ deleted, only now: the delete names it after the refresh.
        assert!(b.revoked(&before.upstream) && b.revoked(&before.held), "round {i}");
        assert_eq!(b.deleted.borrow().last(), Some(&before.held), "round {i}: DELETE .../token named the held ghu_");
        let calls = b.calls.borrow();
        let at = |p: &str, m: &str| calls.iter().rposition(|(cm, cp)| cm == m && cp.ends_with(p)).unwrap();
        assert!(
            at("/login/oauth/access_token", "POST") < at("/token", "DELETE"),
            "round {i}: deleted after the refresh"
        );
        drop(calls);
        assert!(!b.revoked(&next.held) && !b.revoked(&next.upstream));
        rt = v["refresh_token"].as_str().unwrap().to_owned();
        kept += 1;
    }
    assert_eq!(kept, 100);
    // Narrowing is allowed; widening never is, and a refused widening spends nothing.
    for wide in ["write:write_approve_merge", "write:write tools:issues,checks,repos", "write:admin", "repo"] {
        let r = refresh(&b, &rt, Some(wide));
        assert_eq!((r.status, json(&r)["error"].as_str()), (400, Some("invalid_scope")), "{wide}");
    }
    let r = refresh(&b, &rt, Some("write:read tools:issues"));
    assert_eq!((r.status, json(&r)["scope"].as_str()), (200, Some("write:read tools:issues")));
}

#[test]
fn l008_a_replayed_refresh_revokes_the_grant() {
    let b = Bed::new();
    let v = json(&redeem(&b, &code_for(&b, 1, "write:read")));
    let first = v["refresh_token"].as_str().unwrap().to_owned();
    let v2 = json(&refresh(&b, &first, None));
    let (at2, second) =
        (v2["access_token"].as_str().unwrap().to_owned(), v2["refresh_token"].as_str().unwrap().to_owned());
    let upstream_calls = |b: &Bed| b.calls.borrow().iter().filter(|(_, p)| p == "/login/oauth/access_token").count();
    let before = upstream_calls(&b);
    let r = refresh(&b, &first, None);
    assert_eq!((r.status, json(&r)["error"].as_str()), (400, Some("invalid_grant")));
    assert_eq!(upstream_calls(&b), before, "the replay never reached the upstream refresh");
    assert!(b.identity_fold(b.user(1)).grants().values().all(|g| g.revoked), "the grant is revoked");
    // The newest refresh token of the chain is refused too, and the live access token's upstream is dead.
    assert_eq!(refresh(&b, &second, None).status, 400);
    let t: McpToken = sealer().open(Sealed::Access, &at2).unwrap();
    let held: McpRefresh = sealer().open(Sealed::Refresh, &second).unwrap();
    assert!(b.revoked(&t.upstream) && b.revoked(&held.held) && b.revoked(&held.upstream));
}

#[test]
fn l008_framed_cross_site_or_skipped_consent_issues_no_code() {
    let b = Bed::new();
    let page = b.serve(get(&authorize_path("write:read")));
    assert_eq!(page.header("content-security-policy").map(|c| c.contains("frame-ancestors 'none'")), Some(true));
    assert_eq!(page.header("x-frame-options"), Some("DENY"));
    assert!(page.header("set-cookie").is_none(), "no cookie before consent");
    let p = pending(&page);
    let form = [("p", p.as_str()), ("decision", "allow"), ("repos", "acme/mono")];
    // Framed or cross-site submissions carry another Origin, none, or Sec-Fetch-Site: cross-site.
    for h in [
        vec![],
        vec![("origin", "https://evil.test")],
        vec![("origin", ORIGIN), ("sec-fetch-site", "cross-site")],
        vec![("origin", "null")],
    ] {
        let r = b.serve(post("/consent", &form, &h));
        assert_eq!(r.status, 403, "{h:?}");
        assert!(r.header("location").is_none() && r.header("set-cookie").is_none(), "no redirect, no cookie: {h:?}");
    }
    // Skipped consent: straight to the host's OAuth with a state graft-mcp never bound, then the callback with no cookie.
    let skip = b.host_authorize(&format!(
        "{}/login/oauth/authorize?client_id={}&redirect_uri={ORIGIN}/callback&state=forged",
        bed::HOST,
        b.client_id()
    ));
    let r = b.serve(get(&skip));
    assert_eq!(r.status, 400);
    assert!(r.header("location").is_none());
    // Another flow's cookie, and a replayed callback with its own cookie, issue nothing either.
    let ok = b.serve(post("/consent", &form, &same_origin()));
    let cookie = ok.header("set-cookie").unwrap().split(';').next().unwrap().to_owned();
    let theirs = b.serve(post("/consent", &form, &same_origin()));
    let mut cb = get(&b.host_authorize(theirs.header("location").unwrap()));
    cb.headers.push(("cookie".into(), cookie.clone()));
    assert_eq!(b.serve(cb).status, 400, "another consent's cookie");
    let mut cb = get(&b.host_authorize(ok.header("location").unwrap()));
    cb.headers.push(("cookie".into(), cookie));
    assert_eq!(b.serve(cb.clone()).status, 302, "the control");
    let replay = b.serve(cb);
    assert_eq!(replay.status, 400, "the host's code spends once");
    assert!(replay.header("location").is_none());
    // Denying sends the client access_denied with iss, and no code.
    let r = b.serve(post("/consent", &[("p", p.as_str()), ("decision", "deny")], &same_origin()));
    let q = query(r.header("location").unwrap());
    assert_eq!((q["error"].as_str(), q.get("code")), ("access_denied", None));
}

#[test]
fn authorize_refuses_before_consent_on_cimd_pkce_resource_and_scope() {
    let b = Bed::new();
    let plain = authorize_path("write:read").replace("code_challenge_method=S256", "code_challenge_method=plain");
    let back = |r: &Resp| query(r.header("location").unwrap())["error"].clone();
    assert_eq!(back(&b.serve(get(&plain))), "invalid_request");
    let host_res = authorize_path("write:read")
        .replace("resource=https%3A%2F%2Fmcp.git.acme.test%2F", "resource=https%3A%2F%2Fgit.acme.test%2F");
    assert_eq!(back(&b.serve(get(&host_res))), "invalid_target");
    assert_eq!(back(&b.serve(get(&authorize_path("write:admin")))), "invalid_scope");
    // CIMD: each is shown, never redirected, and makes no consent page.
    let doc = |v: Value| v.to_string().into_bytes();
    // Each document is good but for the one rule under test, so no other check refuses it first.
    let ok_doc = doc(serde_json::json!({ "client_id": CIMD, "redirect_uris": [REDIRECT] }));
    // Valid JSON padded past 5 KB, so a reader that stops at the cap still holds a parseable document.
    let mut big = ok_doc.clone();
    big.extend(vec![b' '; 5_200]);
    let cases: Vec<(u16, Option<&str>, Vec<u8>)> = vec![
        (302, None, ok_doc.clone()),
        (301, None, ok_doc),
        (200, None, big),
        (
            200,
            None,
            doc(serde_json::json!({ "client_id": "https://client.example/other.json", "redirect_uris": [REDIRECT] })),
        ),
        (200, None, doc(serde_json::json!({ "client_id": CIMD, "redirect_uris": [] }))),
        (200, None, b"not json".to_vec()),
    ];
    for (status, cc, body) in cases {
        b.cimd_cache.borrow_mut().clear();
        *b.cimd.borrow_mut() = graftyard_mcp::Fetched { status, cache_control: cc.map(Into::into), body };
        let r = b.serve(get(&authorize_path("write:read")));
        assert_eq!(r.status, 400, "{status}: {}", html(&r));
        assert!(r.header("location").is_none());
    }
    for id in [
        "http://client.example/c.json",
        "https://client.example/",
        "https://mcp.git.acme.test/c.json",
        "https://acme.test/c.json",
    ] {
        let p = authorize_path("write:read").replace(
            &url::form_urlencoded::byte_serialize(CIMD.as_bytes()).collect::<String>(),
            &url::form_urlencoded::byte_serialize(id.as_bytes()).collect::<String>(),
        );
        assert_eq!(b.serve(get(&p)).status, 400, "{id}");
    }
    let unregistered = authorize_path("write:read").replace("33418", "1");
    *b.cimd.borrow_mut() = Bed::new().cimd.into_inner();
    b.cimd_cache.borrow_mut().clear();
    assert_eq!(b.serve(get(&unregistered)).status, 400);
    // Cached by its max-age: the second authorize fetches nothing, and past it the document is fetched again.
    b.cimd_cache.borrow_mut().clear();
    let n = b.cimd_fetches.get();
    assert_eq!(b.serve(get(&authorize_path("write:read"))).status, 200);
    assert_eq!(b.serve(get(&authorize_path("write:read"))).status, 200);
    assert_eq!(b.cimd_fetches.get(), n + 1);
    b.now.set(b.now.get() + 301_000);
    assert_eq!(b.serve(get(&authorize_path("write:read"))).status, 200);
    assert_eq!(b.cimd_fetches.get(), n + 2);
}

#[test]
fn consent_page_names_the_client_host_redirect_hosts_and_warns_for_localhost() {
    let b = Bed::new();
    let page = html(&b.serve(get(&authorize_path("write:write"))));
    assert!(page.contains("<code>client.example</code>") && page.contains("<code>127.0.0.1</code>"));
    assert!(page.contains("Warning:</b> every redirect goes to this computer"));
    assert!(page.contains("Write scope: <b>write</b>"));
    // A name with markup is escaped, never rendered.
    *b.cimd.borrow_mut() = graftyard_mcp::Fetched {
        status: 200,
        cache_control: None,
        body: serde_json::json!({ "client_id": CIMD, "client_name": "<script>x</script>", "redirect_uris": [REDIRECT, "https://app.example/cb"] }).to_string().into_bytes(),
    };
    b.cimd_cache.borrow_mut().clear();
    let page = html(&b.serve(get(&authorize_path("write:read"))));
    assert!(!page.contains("<script>") && page.contains("&#60;script&#62;"));
    assert!(!page.contains("Warning:"), "one redirect host is not this computer");
}

#[test]
fn settings_lists_and_revokes_the_users_grants() {
    let b = Bed::new();
    let v = json(&redeem(&b, &code_for(&b, 0, "write:read")));
    let at = v["access_token"].as_str().unwrap().to_owned();
    // Signed out: the host's OAuth signs the user in, bound to state by the same kind of cookie.
    let r = b.serve(get("/settings"));
    assert_eq!(r.status, 302);
    let cookie = r.header("set-cookie").unwrap().split(';').next().unwrap().to_owned();
    let mut cb = get(&b.host_authorize(r.header("location").unwrap()));
    cb.headers.push(("cookie".into(), cookie));
    let r = b.serve(cb);
    let session = r
        .headers
        .iter()
        .filter(|(k, _)| k == "set-cookie")
        .map(|(_, v)| v.split(';').next().unwrap().to_owned())
        .find(|c| c.starts_with("__Host-graft_mcp_session="))
        .unwrap();
    let mut s = get("/settings");
    s.headers.push(("cookie".into(), session.clone()));
    let page = b.serve(s.clone());
    assert_eq!(page.header("x-frame-options"), Some("DENY"));
    let h = html(&page);
    assert!(h.contains(CIMD), "{h}");
    let at_id = h.find("name=grant value=\"").unwrap() + 18;
    let id = h[at_id..at_id + h[at_id..].find('"').unwrap()].to_owned();
    // Revoke needs our origin.
    let mut cross = post("/settings/revoke", &[("grant", &id)], &[("origin", "https://evil.test")]);
    cross.headers.push(("cookie".into(), session.clone()));
    assert_eq!(b.serve(cross).status, 403);
    let mut rv = post("/settings/revoke", &[("grant", &id)], &same_origin());
    rv.headers.push(("cookie".into(), session));
    assert_eq!(b.serve(rv).status, 302);
    let t: McpToken = sealer().open(Sealed::Access, &at).unwrap();
    assert!(b.revoked(&t.upstream), "revoking at settings deletes the client's upstream tokens");
    assert!(!html(&b.serve(s)).contains(CIMD));
}
