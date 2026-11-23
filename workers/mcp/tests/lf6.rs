//! LF-6 (`build/BUILD-PLAN.md`, AM2-WP1), in memory over real members: the host's `token/scoped` and grant endpoints
//! (`graftyard_api::mcp`) as the edge routes them, each case 100 of 100.
//!
//! - `token/scoped` from `Graftyard MCP` without `graftyard_grant`, with another user's grant, and with a
//!   `graftyard_write` above the grant: 422 each, and no scoped token stored.
//! - A stale `generation` on `POST /mcp/grants/{id}/generation`: 409, the grant revoked, and its tokens with it.
//! - Grant calls with wrong client credentials: 401.
//!
//! Controls: a good request answers 200 with GHES's `authorization` (validated against the pinned description when
//! `$GRAFTYARD_REPLIES` is set), and another client's `token/scoped` gets the edge's 404 with or without the fields.

#[path = "common/bed.rs"]
mod bed;
#[path = "../../../crates/identity/tests/common/mod.rs"]
mod common;

use bed::{Bed, CLIENT_SECRET, OWNER_LOGIN, REPO, REPO_NAME, body_json};
use graftyard_identity::{
    GrantId, IdentityCmd, IdentityErr, IdentityOp, OpaqueToken, RepoScope, TokenKind, WriteScope,
};
use graftyard_raft::CommitError;
use graftyard_types::{AccountId, CredentialKey, CredentialShard, DbId};
use serde_json::{Value, json};
use smallvec::smallvec;

const N: usize = 100;

/// A grant for user `i` through `POST /mcp/grants`, and the unscoped `ghu_` that made it.
fn grant(b: &Bed, i: usize, write: &str) -> (String, String) {
    b.browser.set(i);
    let (ghu, _) = b.issue_pair(b.user(i));
    let body = json!({ "client": bed::CIMD, "redirect_hosts": ["127.0.0.1"], "write": write });
    let r = b.grants("POST", "/mcp/grants", Some(&b.basic(CLIENT_SECRET)), Some(&ghu), &body);
    assert_eq!(r.status, 201, "{:?}", r.body);
    (body_json(&r)["id"].as_str().unwrap().to_owned(), ghu)
}

fn scoped_body(ghu: &str, grant: Option<&str>, write: Option<&str>) -> Value {
    let mut v = json!({ "access_token": ghu, "target": OWNER_LOGIN, "repositories": [REPO_NAME] });
    if let Some(g) = grant {
        v["graftyard_grant"] = json!(g);
    }
    if let Some(w) = write {
        v["graftyard_write"] = json!(w);
    }
    v
}

fn keep_rest(case: &str, status: u16, body: &Value) {
    if let Ok(dir) = std::env::var("GRAFTYARD_REPLIES") {
        let v = json!({ "method": "POST", "path": "/applications/{client_id}/token/scoped", "status": status, "body": body });
        std::fs::write(format!("{dir}/{case}.rest.json"), serde_json::to_vec_pretty(&v).unwrap()).unwrap();
    }
}

/// Every scoped token row the user holds now, live or not: a refused request must have stored none.
fn scoped_rows(b: &Bed, u: AccountId) -> usize {
    b.identity_fold(u).grants().values().map(|r| r.tokens.len()).sum()
}

#[test]
fn lf6_every_bad_scoped_request_is_422_and_stores_no_token() {
    let b = Bed::new();
    let auth = b.basic(CLIENT_SECRET);
    let client = b.client_id();
    let (alice_grant, _) = grant(&b, 0, "write");
    let (bob_grant, _) = grant(&b, 1, "write");
    let (read_grant, _) = grant(&b, 0, "read");
    let before = (scoped_rows(&b, b.user(0)), scoped_rows(&b, b.user(1)));
    let mut counts = [0usize; 4];
    for i in 0..N {
        b.browser.set(0);
        let (ghu, _) = b.issue_pair(b.user(0));
        // Asymmetric on purpose: each request is good in every respect but the one under test.
        let cases: [(Value, &str); 4] = [
            (scoped_body(&ghu, None, Some("read")), "graftyard_grant"),
            (scoped_body(&ghu, Some(&bob_grant), Some("read")), "graftyard_grant"),
            (
                scoped_body(&ghu, Some(&read_grant), Some(if i % 2 == 0 { "write" } else { "write_approve_merge" })),
                "graftyard_write",
            ),
            (scoped_body(&ghu, Some(&alice_grant), None), "graftyard_write"),
        ];
        for (k, (body, field)) in cases.iter().enumerate() {
            let r = b.scoped(&client, Some(&auth), body).expect("Graftyard MCP's token/scoped is served");
            assert_eq!(r.status, 422, "case {k}, round {i}: {:?}", r.body);
            let e = body_json(&r);
            assert_eq!(e["errors"][0]["field"], *field, "case {k}");
            let code = if k == 0 || k == 3 { "missing_field" } else { "invalid" };
            assert_eq!(e["errors"][0]["code"], code, "case {k}");
            assert!(!e.to_string().contains(&format!("{}", b.user(1).0.0)), "the 422 names no other user");
            keep_rest(&format!("lf6-422-{k}"), r.status, &e);
            counts[k] += 1;
        }
    }
    assert_eq!(counts, [N; 4], "422 for each bad request, 100 of 100");
    assert_eq!((scoped_rows(&b, b.user(0)), scoped_rows(&b, b.user(1))), before, "no refused request stored a token");
    // The control: the same token, Alice's own grant, a write within it.
    let (ghu, _) = b.issue_pair(b.user(0));
    let r = b.scoped(&client, Some(&auth), &scoped_body(&ghu, Some(&alice_grant), Some("write"))).unwrap();
    assert_eq!(r.status, 200, "{:?}", r.body);
    let v = body_json(&r);
    keep_rest("lf6-200", 200, &v);
    let tok = v["token"].as_str().unwrap();
    assert!(tok.starts_with("ghu_") && tok != ghu);
    let (row, fields) = b.row(tok).expect("the scoped token is stored");
    let f = fields.unwrap();
    assert_eq!((row.kind, row.account, row.revoked), (TokenKind::UserToServer, b.user(0), false));
    let g = bed_grant_id(&alice_grant);
    assert_eq!((f.grant, f.write, f.resource_owner), (Some(g), Some(WriteScope::Write), Some(b.owner)));
    assert_eq!(f.repos, Some(RepoScope::Selected(smallvec![REPO])));
    assert!(row.expires.is_some_and(|e| e.0 <= b.now.get() + 3_600_000), "an hour at most");
    let f0 = b.identity_fold(b.user(0));
    let rows = &f0.grants()[&g];
    assert!(rows.tokens.contains_key(&OpaqueToken::parse(tok).unwrap().hash()), "recorded under the grant");
}

/// A revocation that commits after the edge read the grant: the edge's check passes on its stale read, the
/// `Identity` leader refuses `McpScoped`, and the scoped token already stored is revoked, so the 422 leaves none live.
#[test]
fn lf6_a_scoped_token_raced_by_a_revocation_is_refused_and_left_revoked() {
    let b = Bed::new();
    let auth = b.basic(CLIENT_SECRET);
    let (id, _) = grant(&b, 0, "write");
    *b.stale_identity.borrow_mut() = Some(b.identity_fold(b.user(0)));
    let r =
        b.grants("DELETE", &format!("/mcp/grants/{id}"), Some(&auth), Some(&b.issue_pair(b.user(0)).0), &Value::Null);
    assert_eq!(r.status, 204);
    let before: std::collections::BTreeSet<_> = live_scoped(&b);
    let (ghu, _) = b.issue_pair(b.user(0));
    let r = b.scoped(&b.client_id(), Some(&auth), &scoped_body(&ghu, Some(&id), Some("read"))).unwrap();
    assert_eq!(r.status, 422, "{:?}", r.body);
    let after = live_scoped(&b);
    assert_eq!(before, after, "no scoped token of the revoked grant is live");
}

/// Every live `ghu_` that carries a grant, across the shards the bed touched.
fn live_scoped(b: &Bed) -> std::collections::BTreeSet<[u8; 32]> {
    let w = b.w.borrow();
    let mut out = std::collections::BTreeSet::new();
    for g in w.tokens.values() {
        let s = g.leader().expect("a leader");
        let f = &g.m(s).group_fold().fold;
        for (h, row) in &f.tokens {
            if !row.revoked && f.more.fields.get(h).is_some_and(|x| x.grant.is_some()) {
                out.insert(*h);
            }
        }
    }
    out
}

fn bed_grant_id(wire: &str) -> GrantId {
    GrantId(DbId(wire.split_once('.').unwrap().1.parse().unwrap()))
}

/// The commit point binds too: an `McpScoped` the edge checked against a stale read is refused on the leader.
#[test]
fn lf6_the_identity_leader_refuses_a_scoped_token_above_or_outside_its_grant() {
    let b = Bed::new();
    let (read_grant, _) = grant(&b, 0, "read");
    let g = bed_grant_id(&read_grant);
    let (ghu, _) = b.issue_pair(b.user(0));
    let t = OpaqueToken::parse(&ghu).unwrap();
    let cred = CredentialShard {
        account: Some(b.user(0)),
        key: Some(CredentialKey::Token { shard: t.shard(), hash: t.hash() }),
    };
    let scoped =
        |write, grant| IdentityOp::McpScoped { grant, write, token: [9; 32], expires: graftyard_types::Timestamp(1) };
    let refuse = |op| match b.w.borrow_mut().identity_cmd(b.user(0), IdentityCmd { cred, op }) {
        Err(CommitError::Refused { err, .. }) => err,
        other => panic!("{other:?}"),
    };
    assert_eq!(refuse(scoped(WriteScope::Write, g)), IdentityErr::WriteAbove);
    assert_eq!(refuse(scoped(WriteScope::Read, GrantId(DbId(99)))), IdentityErr::NoGrant);
    // Bob cannot record a token under Alice's grant, even naming her account.
    let (bob_ghu, _) = b.issue_pair(b.user(1));
    let bt = OpaqueToken::parse(&bob_ghu).unwrap();
    let bob = CredentialShard {
        account: Some(b.user(1)),
        key: Some(CredentialKey::Token { shard: bt.shard(), hash: bt.hash() }),
    };
    match b.w.borrow_mut().identity_cmd(b.user(0), IdentityCmd { cred: bob, op: scoped(WriteScope::Read, g) }) {
        Err(CommitError::Refused { .. }) => {}
        other => panic!("{other:?}"),
    }
    assert!(b.identity_fold(b.user(0)).grants()[&g].tokens.is_empty());
}

#[test]
fn lf6_a_stale_generation_is_409_and_revokes_the_grant_and_its_tokens() {
    let b = Bed::new();
    let auth = b.basic(CLIENT_SECRET);
    let client = b.client_id();
    let mut revoked = 0;
    for i in 0..N {
        let who = i % 2;
        let (id, ghu) = grant(&b, who, "write");
        let r = b.scoped(&client, Some(&auth), &scoped_body(&ghu, Some(&id), Some("read"))).unwrap();
        assert_eq!(r.status, 200);
        let tok = body_json(&r)["token"].as_str().unwrap().to_owned();
        let path = format!("/mcp/grants/{id}/generation");
        // The current generation moves it on; then every older or newer one is stale.
        let ok = b.grants("POST", &path, Some(&auth), None, &json!({ "generation": 0 }));
        assert_eq!((ok.status, body_json(&ok)["generation"].as_u64()), (200, Some(1)), "{:?}", ok.body);
        let stale = [0u32, 7, 2][i % 3];
        let r = b.grants("POST", &path, Some(&auth), None, &json!({ "generation": stale }));
        assert_eq!(r.status, 409, "round {i}: {:?}", r.body);
        assert_eq!(body_json(&r)["revoked"], true);
        let fw = b.identity_fold(b.user(who));
        let g = &fw.grants()[&bed_grant_id(&id)];
        assert!(g.revoked, "the grant row is revoked");
        assert!(b.revoked(&tok), "its scoped token is revoked");
        assert!(b.revoked(&ghu), "and the unscoped token it narrowed, with its pair");
        // Revoked stays revoked: the right generation is refused now, and no new scoped token is minted.
        let again = b.grants("POST", &path, Some(&auth), None, &json!({ "generation": 1 }));
        assert_eq!(again.status, 409);
        let (fresh, _) = b.issue_pair(b.user(who));
        let r = b.scoped(&client, Some(&auth), &scoped_body(&fresh, Some(&id), Some("read"))).unwrap();
        assert_eq!(r.status, 422, "a revoked grant scopes nothing");
        let list = b.grants("GET", "/mcp/grants", Some(&auth), Some(&fresh), &Value::Null);
        assert!(!body_json(&list).as_array().unwrap().iter().any(|x| x["id"] == id), "and is no longer listed");
        revoked += 1;
    }
    assert_eq!(revoked, N, "409 and revoked, 100 of 100");
}

#[test]
fn lf6_grant_calls_with_wrong_client_credentials_are_401() {
    let b = Bed::new();
    let (id, ghu) = grant(&b, 0, "write");
    let other = graftyard_identity::ClientId::for_app(graftyard_identity::AppId(DbId(77)));
    let b64 = |s: String| format!("Basic {}", base64_std(&s));
    let wrong: Vec<Option<String>> = vec![
        None,
        Some(b.basic("0000000000000000000000000000000000000000")),
        Some(b.basic("")),
        Some(b64(format!("{}:{CLIENT_SECRET}", other.as_str()))),
        Some(format!("Bearer {ghu}")),
        Some("Basic !!!".into()),
        Some(b64(b.client_id())),
    ];
    let calls: [(&str, String, Value); 4] = [
        ("GET", "/mcp/grants".into(), Value::Null),
        ("POST", "/mcp/grants".into(), json!({ "client": bed::CIMD, "redirect_hosts": ["127.0.0.1"] })),
        ("POST", format!("/mcp/grants/{id}/generation"), json!({ "generation": 0 })),
        ("DELETE", format!("/mcp/grants/{id}"), Value::Null),
    ];
    let mut n = 0;
    for i in 0..N {
        let auth = &wrong[i % wrong.len()];
        let (m, p, body) = &calls[i % calls.len()];
        let r = b.grants(m, p, auth.as_deref(), Some(&ghu), body);
        assert_eq!(r.status, 401, "{m} {p} with {auth:?}: {:?}", r.body);
        n += 1;
    }
    assert_eq!(n, N, "401 for wrong credentials, 100 of 100");
    // None of them changed the grant: its generation still spends once.
    let f0 = b.identity_fold(b.user(0));
    let g = &f0.grants()[&bed_grant_id(&id)];
    assert!(!g.revoked && g.grant.generation == 0);
    // The right client credentials with a user token that is not a live `ghu_` of `Graftyard MCP`: 401 on the grant
    // endpoints, and 404 on `token/scoped`, as GHES answers an invalid token.
    let other_app = b.other_app_token(b.user(0), graftyard_identity::AppId(DbId(77)), other);
    let good = b.basic(CLIENT_SECRET);
    for user in [other_app.as_str(), "ghp_notatoken", ""] {
        let r = b.grants("GET", "/mcp/grants", Some(&good), Some(user), &Value::Null);
        assert_eq!(r.status, 401, "user token {user:?}");
        let r = b.scoped(&b.client_id(), Some(&good), &scoped_body(user, Some(&id), Some("read"))).unwrap();
        // An empty `access_token` is a missing field, which is 422.
        assert_eq!(r.status, if user.is_empty() { 422 } else { 404 }, "token/scoped of {user:?}");
    }
    // `token/scoped` with wrong credentials is GHES's 401 too.
    let r = b.scoped(&b.client_id(), Some(&b.basic("nope")), &scoped_body(&ghu, Some(&id), Some("read"))).unwrap();
    assert_eq!(r.status, 401);
}

fn base64_std(s: &str) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(s)
}

/// No GHES response changes for a client that does not opt in: another App's `token/scoped`, with or without the
/// additive fields, gets the 404 the edge answered before, and the grant endpoints are under `/api/graftyard/v1`.
#[test]
fn another_clients_token_scoped_is_unchanged() {
    let b = Bed::new();
    let other = graftyard_identity::ClientId::for_app(graftyard_identity::AppId(DbId(77)));
    let (ghu, _) = b.issue_pair(b.user(0));
    for body in [scoped_body(&ghu, None, None), scoped_body(&ghu, Some("1.1"), Some("read"))] {
        assert!(b.scoped(other.as_str(), Some(&b.basic(CLIENT_SECRET)), &body).is_none(), "the edge's own 404");
    }
    assert!(graftyard_api::mcp::route("/agents").is_none());
    assert_eq!(graftyard_api::mcp::route("/mcp/grants/x"), Some(graftyard_api::mcp::Route::Unknown));
}
