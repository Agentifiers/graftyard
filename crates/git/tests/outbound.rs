//! AM3-WP2's outbound router in memory (`crates/git/src/outbound.rs`): M0-WP14's L0-02 attempt classes, each variant
//! that reaches the router, replayed against `outbound` in fork mode and in-repo, beside the positive controls that
//! must still pass. Variants that never reach the router (IP literals, other ports' raw TCP, DNS) are the platform's
//! (`tests/m0/results/m0-wp14.md`, "What the Sandbox SDK does and does not give the design").

use std::io::Write;

use graftyard_git::controller::{RepoRef, TASK_WALL, Task, TaskBudget};
use graftyard_git::outbound::{Allowed, Arm, Cred, OutReq, Route, Scheme, go_module, outbound, receive_refs};
use graftyard_git::pkt;
use graftyard_identity::{Login, RepoName, SessionId, UserId, WorkspaceMode};
use graftyard_types::{AccountId, DbId, Index};

const HOST: &str = "git.acme.test";
const MODEL: &str = "model.test";
const GO: &str = "goproxy.test";
const RUN: u128 = 0x0e6a_1b2c_0000_4000_8000_0000_0000_0001;
const PREFIX: &str = "refs/heads/graftyard/0e6a1b2c-0000-4000-8000-000000000001/";
const SHA: &str = "1111111111111111111111111111111111111111";

fn route(mode: WorkspaceMode) -> Route {
    let base = RepoRef { owner: Login::parse("acme").unwrap(), name: RepoName::parse("app").unwrap() };
    let budget = TaskBudget { task_tokens: 1_000, org_month_tokens: 50_000, wall: TASK_WALL };
    let alice = UserId(AccountId(DbId(7)));
    let t = Task::new(SessionId::Run(RUN), None, Some(Index(4)), alice, base, mode, budget).unwrap();
    let others = vec![
        Allowed::ModelInference { host: MODEL.into(), path: "/v1/messages".into() },
        Allowed::Registry { host: GO.into(), go_modules: Some(vec!["github.com/graftyard/known".into()]) },
        Allowed::Registry { host: "npm.mirror.test".into(), go_modules: None },
    ];
    Route::of(&t, HOST, others).unwrap()
}

fn hdrs(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

/// The rule that refused, or `"forwarded"`.
fn rule(r: &Route, method: &str, url: &str, headers: &[(&str, &str)], body: &[u8]) -> &'static str {
    let h = hdrs(headers);
    match outbound(OutReq { method, url, headers: &h, body }, r) {
        Ok(_) => "forwarded",
        Err(e) => e.rule,
    }
}

fn receive(refs: &[&str]) -> Vec<u8> {
    let mut b = Vec::new();
    for (i, r) in refs.iter().enumerate() {
        let caps = if i == 0 { "\0report-status" } else { "" };
        pkt::line(&mut b, &format!("{} {SHA} {r}{caps}", "0".repeat(40)));
    }
    pkt::flush(&mut b);
    b.extend_from_slice(b"PACK....");
    b
}

fn gzip(b: &[u8]) -> Vec<u8> {
    let mut z = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    z.write_all(b).unwrap();
    z.finish().unwrap()
}

#[test]
fn l0_02_positive_controls_forward_with_the_hosts_credential_and_never_the_sandboxs() {
    let r = route(WorkspaceMode::Fork);
    let sandbox = [("authorization", "Basic eC1hY2Nlc3MtdG9rZW46Z2hwX1o="), ("cookie", "s=1"), ("host", "evil.test")];
    let h = hdrs(&sandbox);
    let go = |method, url: &str, body: &[u8]| outbound(OutReq { method, url, headers: &h, body }, &r).unwrap();

    // The base clones with graftyard-agent's token, even with a token in the URL's userinfo.
    let f =
        go("GET", &format!("https://x-access-token:ghp_Z@{HOST}/acme/app.git/info/refs?service=git-upload-pack"), b"");
    assert_eq!((f.arm, f.cred), (Arm::GraftyardHost, Cred::Agent(Scheme::Basic)));
    assert_eq!(f.url, format!("https://{HOST}/acme/app.git/info/refs?service=git-upload-pack"), "userinfo is gone");
    assert!(f.headers.iter().all(|(k, _)| !matches!(k.as_str(), "authorization" | "cookie" | "host")));
    // A push inside the task's refs on the fork carries graftyard-workspace's token.
    let body = receive(&[&format!("{PREFIX}feat/1")]);
    let f = go("POST", &format!("https://{HOST}/Acme-Agents/App.git/git-receive-pack"), &body);
    assert_eq!(f.cred, Cred::Workspace(Scheme::Basic), "matched case-insensitively");
    // REST on the base reads with graftyard-agent's token; a status read is a read.
    assert_eq!(
        go("GET", &format!("https://{HOST}/api/v3/repos/acme/app/pulls"), b"").cred,
        Cred::Agent(Scheme::Bearer)
    );
    assert_eq!(
        go("GET", &format!("https://{HOST}/api/v3/repos/acme/app/statuses/{SHA}"), b"").cred,
        Cred::Agent(Scheme::Bearer)
    );
    // A clean model call carries the model key and the body the router parsed, re-serialized.
    let f = go(
        "POST",
        &format!("https://{MODEL}/v1/messages"),
        br#"{"model":"m",  "messages":[],"tools":[{"type":"function"},{"name":"c"}]}"#,
    );
    assert_eq!((f.arm, f.cred), (Arm::ModelInference, Cred::ModelKey));
    assert_eq!(
        f.body.as_deref(),
        Some(&br#"{"messages":[],"model":"m","tools":[{"type":"function"},{"name":"c"}]}"#[..])
    );
    // The Sandbox picks neither the key nor the API version: its `x-api-key`, `anthropic-version` and
    // `Authorization` are dropped, and the host sets Anthropic's two headers, never `Authorization: Bearer`.
    let own = hdrs(&[
        ("authorization", "Bearer sk-sandbox"),
        ("x-api-key", "sk-ant-sandbox"),
        ("anthropic-version", "2099-01-01"),
        ("anthropic-beta", "b1"),
    ]);
    let body = br#"{"model":"m","messages":[]}"#;
    let f = outbound(OutReq { method: "POST", url: &format!("https://{MODEL}/v1/messages"), headers: &own, body }, &r)
        .unwrap();
    let names: Vec<&str> = f.headers.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(names, ["anthropic-beta", "content-type"], "only the Sandbox's other headers pass");
    assert_eq!(
        Cred::model_headers("sk-ant-host"),
        [("x-api-key", "sk-ant-host".to_owned()), ("anthropic-version", "2023-06-01".to_owned())]
    );
    // The listed Go module, and a plain registry GET.
    assert_eq!(go("GET", &format!("https://{GO}/github.com/graftyard/known/@latest"), b"").cred, Cred::None);
    assert_eq!(go("GET", "https://npm.mirror.test/left-pad", b"").arm, Arm::Registry);
}

#[test]
fn l0_02_unlisted_hosts_plain_http_other_ports_and_connect_are_refused() {
    let r = route(WorkspaceMode::Fork);
    for (url, want) in [
        ("https://canary.test/probe", "UnlistedHost"),
        ("https://canary.test./probe", "UnlistedHost"),
        ("https://192.0.2.1/probe", "UnlistedHost"),
        ("https://[2001:db8::1]/probe", "UnlistedHost"),
        ("https://git.acme.test.canary.test/acme/app.git/info/refs", "UnlistedHost"),
        ("https://evilgit.acme.test/acme/app.git/info/refs", "UnlistedHost"),
        ("https://x.model.test/v1/messages", "UnlistedHost"),
        ("http://git.acme.test/acme/app.git/info/refs", "PlainHttp"),
        ("https://canary.test:8443/probe", "Port"),
        ("https://git.acme.test:8443/acme/app.git/info/refs", "Port"),
    ] {
        assert_eq!(rule(&r, "GET", url, &[], b""), want, "{url}");
    }
    assert_eq!(rule(&r, "CONNECT", "https://git.acme.test/", &[], b""), "Connect");
    // A trailing dot on an allowed name is the same host; `Host:` naming another is dropped, never followed.
    assert_eq!(
        rule(&r, "GET", "https://GIT.ACME.TEST./acme/app.git/HEAD", &[("host", "canary.test")], b""),
        "forwarded"
    );
}

#[test]
fn l0_02_a_push_to_another_repository_the_base_or_outside_the_tasks_refs_is_refused() {
    let fork = route(WorkspaceMode::Fork);
    let inside = receive(&[&format!("{PREFIX}feat/1")]);
    for (path, want) in [
        ("/acme/secrets.git/git-receive-pack", "OtherRepo"),
        ("/ACME/Secrets.git/git-receive-pack", "OtherRepo"),
        ("/acme/app.git.evil/git-receive-pack", "OwnerPath"),
        ("/api/v3/repos/acme/secrets/git/refs", "OtherRepo"),
        ("/acme/app.git/git-receive-pack", "ReceivePackBase"),
    ] {
        assert_eq!(rule(&fork, "POST", &format!("https://{HOST}{path}"), &[], &inside), want, "{path}");
    }
    let adv = format!("https://{HOST}/acme/app.git/info/refs?service=git-receive-pack");
    assert_eq!(rule(&fork, "GET", &adv, &[], b""), "ReceivePackBase", "the base's receive-pack advertisement");
    let target = format!("https://{HOST}/acme-agents/app.git/git-receive-pack");
    for refs in [
        vec!["refs/heads/graftyard/other-task/x".to_owned()],
        vec!["refs/heads/main".to_owned()],
        vec![PREFIX.trim_end_matches('/').to_owned()],
        vec![format!("{PREFIX}ok"), "refs/heads/main".to_owned()],
        vec![],
    ] {
        let refs: Vec<&str> = refs.iter().map(String::as_str).collect();
        assert_eq!(rule(&fork, "POST", &target, &[], &receive(&refs)), "OutsidePrefix", "{refs:?}");
    }
    let gz = gzip(&receive(&["refs/heads/main"]));
    assert_eq!(rule(&fork, "POST", &target, &[("content-encoding", "gzip")], &gz), "OutsidePrefix", "gzip is read");
    assert_eq!(rule(&fork, "POST", &target, &[], b"00zzbad"), "ReceivePackParse");
    // In-repo, the base is the push target: inside the refs it pushes with graftyard-workspace's token.
    let inrepo = route(WorkspaceMode::InRepo);
    let base = format!("https://{HOST}/acme/app.git/git-receive-pack");
    let h = hdrs(&[]);
    let f = outbound(OutReq { method: "POST", url: &base, headers: &h, body: &inside }, &inrepo).unwrap();
    assert_eq!(f.cred, Cred::Workspace(Scheme::Basic));
    assert_eq!(rule(&inrepo, "POST", &base, &[], &receive(&["refs/heads/main"])), "OutsidePrefix");
    assert_eq!(
        rule(&inrepo, "POST", &format!("https://{HOST}/acme-agents/app.git/git-receive-pack"), &[], &inside),
        "OtherRepo"
    );
}

#[test]
fn l0_02_gists_graphql_and_status_or_check_run_writes_are_refused() {
    let r = route(WorkspaceMode::Fork);
    let post = |path: &str| rule(&r, "POST", &format!("https://{HOST}{path}"), &[], b"{}");
    for (path, want) in [
        ("/gists", "Gist"),
        ("/api/v3/gists", "Gist"),
        ("/GISTS", "Gist"),
        ("/api/v3/repos/acme/app/../../gists", "PathEncoding"),
        ("/%67ists", "PathEncoding"),
        ("/api/v3/repos/acme//app/pulls", "PathEncoding"),
        ("/api/graphql", "GraphQL"),
        ("/graphql", "GraphQL"),
        ("/api/GraphQL", "GraphQL"),
        ("/api/uploads/x", "Upload"),
        ("/api/v3/user/repos", "OwnerPath"),
    ] {
        assert_eq!(post(path), want, "{path}");
    }
    assert_eq!(rule(&r, "GET", &format!("https://{HOST}/graphql?query=x"), &[], b""), "GraphQL");
    for path in [
        format!("/api/v3/repos/acme/app/statuses/{SHA}"),
        format!("/api/v3/repos/acme-agents/app/statuses/{SHA}"),
        "/api/v3/repos/acme/app/check-runs".to_owned(),
        "/api/v3/repos/acme/app/check-suites".to_owned(),
        format!("/api/v3/repos/acme/app/commits/{SHA}/check-runs"),
        format!("/api/v3/repos/acme/app/commits/{SHA}/statuses"),
        format!("/api/v3/repos/acme/app/STATUSES/{SHA}"),
    ] {
        assert_eq!(post(&path), "StatusOrCheckRun", "{path}");
    }
    let get = format!("https://{HOST}/api/v3/repos/acme/app/statuses/{SHA}");
    assert_eq!(rule(&r, "GET", &get, &[("x-http-method-override", "POST")], b""), "MethodOverride");
    // A write that is not a status passes the router; `decide` owns what the token may write.
    assert_eq!(post("/api/v3/repos/acme-agents/app/pulls"), "forwarded");
}

#[test]
fn l0_02_model_requests_with_mcp_servers_a_server_tool_or_another_endpoint_are_refused() {
    let r = route(WorkspaceMode::Fork);
    let url = format!("https://{MODEL}/v1/messages");
    for body in [
        r#"{"model":"m","messages":[],"mcp_servers":[{"url":"https://x"}]}"#,
        r#"{"model":"m","messages":[],"mcp_servers":[{"url":"https://x"}],"mcp_servers":null}"#,
        r#"{"model":"m","messages":[],"mcp\u005fservers":[{"url":"https://x"}]}"#,
        r#"{"model":"m","messages":[],"tools":[{"type":"web_search_20250305"}]}"#,
        r#"{"model":"m","messages":[],"tools":[{"type":"code_execution_20250522"}]}"#,
        r#"{"model":"m","messages":[],"tools":[{"type":"web_fetch"}]}"#,
        r#"{"model":"m","messages":[],"tools":[{"type":"bash_20250124"}]}"#,
        r#"{"model":"m","messages":[],"tools":[{"type":"WEB_SEARCH"}]}"#,
        r#"{"model":"m","messages":[],"tools":[{"type":"Function"}]}"#,
        r#"{"model":"m","messages":[],"tools":[{"type":null}]}"#,
        r#"{"model":"m","messages":[],"builtin_tools":[{"type":"mcp"}]}"#,
        r#"{"model":"m","messages":[],"tools":{"type":"function"}}"#,
        r#"[{"model":"m"}]"#,
    ] {
        assert_eq!(rule(&r, "POST", &url, &[], body.as_bytes()), "ModelPayload", "{body}");
    }
    assert_eq!(rule(&r, "POST", &url, &[], b"{\"model\":"), "ModelBody");
    for (path, want) in [
        ("/v1/files", "ModelFileBatch"),
        ("/v1/batches", "ModelFileBatch"),
        ("/v1/messages/batches", "ModelFileBatch"),
        ("/v1/complete", "ModelPath"),
    ] {
        assert_eq!(rule(&r, "POST", &format!("https://{MODEL}{path}"), &[], b"{}"), want, "{path}");
    }
    assert_eq!(rule(&r, "GET", &url, &[], b""), "ModelMethod");
}

#[test]
fn l0_02_an_unknown_go_module_or_a_registry_query_is_refused() {
    let r = route(WorkspaceMode::Fork);
    for (path, want) in [
        ("/github.com/evil/pkg/@latest", "UnknownGoModule"),
        ("/github.com/evil/pkg/@v/list", "UnknownGoModule"),
        ("/github.com/evil/pkg/@v/v1.0.0.info", "UnknownGoModule"),
        ("/evil.test/mod/@v/v0.1.0.zip", "UnknownGoModule"),
        ("/github.com/!e!v!i!l/pkg/@latest", "UnknownGoModule"),
        ("/github.com/graftyard/knownevil/@latest", "UnknownGoModule"),
        ("/github.com/graftyard/known/sub/@latest", "UnknownGoModule"),
        ("/github.com/Graftyard/known/@latest", "RegistryPath"),
        ("/github.com/graftyard/known/@v/../evil", "PathEncoding"),
        ("/github.com%2Fevil%2Fpkg/@latest", "PathEncoding"),
        ("/github.com/graftyard/known", "RegistryPath"),
    ] {
        assert_eq!(rule(&r, "GET", &format!("https://{GO}{path}"), &[], b""), want, "{path}");
    }
    let q = format!("https://{GO}/github.com/graftyard/known/@latest?go-get=1&upstream=evil");
    assert_eq!(rule(&r, "GET", &q, &[], b""), "RegistryMethod");
    assert_eq!(
        rule(&r, "POST", &format!("https://{GO}/github.com/graftyard/known/@latest"), &[], b""),
        "RegistryMethod"
    );
    assert_eq!(go_module("github.com/!graftyard/known/@v/v1.2.3.mod").as_deref(), Some("github.com/Graftyard/known"));
    assert_eq!(go_module("github.com/!/x/@latest"), None, "! must escape a lowercase letter");
    assert_eq!(
        receive_refs(&receive(&["refs/heads/a", "refs/heads/b"]), false).unwrap(),
        ["refs/heads/a", "refs/heads/b"]
    );
}
