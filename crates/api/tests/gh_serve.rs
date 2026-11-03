//! Serves the seeded repository's GraphQL and statuses route on 127.0.0.1:$GRAFTYARD_SERVE for the pinned gh run in
//! `gh/README.md`, which proves gh's own commands read and write through the handlers. Ignored by default.

mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;

use common::*;
use graftyard_api::exec::Request;
use graftyard_api::graphql::{Plan, plan, query, refused, render, render_merge, route};
use graftyard_api::rest::{Page, list_commit_statuses};
use graftyard_api::wire::Host;
use graftyard_git::issue_ops::IssueOp;
use graftyard_git::issues::{
    AppId, CheckConclusion, CheckRunId, CheckRunUpdate, CheckStatus, CheckSuiteId, CommentTarget, IssueNumber,
    LabelName, StatusId, StatusState, StatusUpdate,
};
use graftyard_types::{AccountId, DbId, Timestamp};
use serde_json::{Value, json};
use smallvec::smallvec;

#[test]
#[ignore]
fn serve() {
    let port: u16 = std::env::var("GRAFTYARD_SERVE").expect("GRAFTYARD_SERVE").parse().unwrap();
    let mut c = Chain::new(1);
    c.push("refs/heads/main", oid(1));
    c.push("refs/heads/f", oid(2));
    c.push("refs/heads/g", oid(3));
    let bug = match c
        .commit(
            WRITER,
            IssueOp::DefineLabel { id: None, name: LabelName("bug".into()), color: [215, 58, 74], description: None },
        )
        .unwrap()
    {
        graftyard_git::issue_ops::IssueOut::Id(id) => graftyard_git::issues::LabelId(id),
        _ => unreachable!(),
    };
    c.commit(
        READER,
        IssueOp::OpenIssue {
            title: "it breaks".into(),
            body: "steps".into(),
            labels: smallvec![bug],
            assignees: smallvec![],
        },
    )
    .unwrap();
    c.commit(
        READER,
        IssueOp::Comment { on: CommentTarget::Issue(IssueNumber(1)), body: "me too".into(), reply_to: None },
    )
    .unwrap();
    c.commit(
        WRITER,
        IssueOp::OpenPr {
            head: graftyard_git::issues::HeadSpec { repo: REPO, branch: name("refs/heads/f") },
            base: name("refs/heads/main"),
            title: "fix it".into(),
            body: "fixes #1".into(),
            draft: false,
            head_sha: oid(2),
            base_sha: oid(1),
        },
    )
    .unwrap();
    c.commit(
        OWNER,
        IssueOp::SetRule {
            id: None,
            pattern: "main".into(),
            protection: Some(graftyard_git::issues::Protection {
                checks: Some(graftyard_git::issues::RequiredChecks {
                    strict: false,
                    checks: smallvec![("ci".into(), None)],
                }),
                reviews: None,
                push: None,
                enforce_admins: false,
                linear_history: false,
                force_pushes: false,
                deletions: false,
                block_creations: false,
                conversation_resolution: false,
                lock_branch: false,
                signatures: false,
                merge_queue: None,
            }),
        },
    )
    .unwrap();
    for (id, n, concl) in [(10, "ci", CheckConclusion::Success), (11, "lint", CheckConclusion::Failure)] {
        c.commit(
            WRITER,
            IssueOp::RecordCheck(CheckRunUpdate {
                run: CheckRunId(DbId(id)),
                suite: CheckSuiteId(DbId(1)),
                app: AppId(DbId(5)),
                head_sha: oid(2),
                name: n.into(),
                version: 1,
                status: CheckStatus::Completed,
                conclusion: Some(concl),
                started: Some(Timestamp(1_793_000_000_000)),
                completed: Some(Timestamp(1_793_000_060_000)),
                external_id: None,
                details_url: Some(graftyard_git::issues::Url("https://ci.example/1".into())),
                output: None,
            }),
        )
        .unwrap();
    }
    c.commit(
        WRITER,
        IssueOp::RecordStatus(StatusUpdate {
            id: StatusId(DbId(12)),
            sha: oid(2),
            context: "jenkins".into(),
            state: StatusState::Pending,
            target_url: None,
            description: Some("building".into()),
            creator: AccountId(DbId(WRITER)),
        }),
    )
    .unwrap();
    let host = Host("ghes.graftyard.test".into());
    let dir = Dir::new(WRITER);
    let l = TcpListener::bind(("127.0.0.1", port)).unwrap();
    for s in l.incoming() {
        let mut s = s.unwrap();
        let mut rd = BufReader::new(s.try_clone().unwrap());
        let mut line = String::new();
        rd.read_line(&mut line).unwrap();
        let mut len = 0;
        loop {
            let mut h = String::new();
            rd.read_line(&mut h).unwrap();
            if h.trim().is_empty() {
                break;
            }
            if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
                len = v.trim().parse().unwrap();
            }
        }
        let mut body = vec![0; len];
        rd.read_exact(&mut body).unwrap();
        let mut parts = line.split_whitespace();
        let (method, path) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
        let (status, out) = if path == "/api/graphql" {
            let b: Value = serde_json::from_slice(&body).unwrap();
            let req = Request::parse(b["query"].as_str().unwrap(), b["variables"].as_object().cloned(), None).unwrap();
            let r = c.repo(0, &host, &dir);
            let reply = match route(&req).unwrap() {
                None => query(&req, None),
                Some(_) if !req.is_mutation().unwrap() => query(&req, Some(&r)),
                Some(_) => match plan(&req, &r) {
                    Err(e) => e,
                    Ok(p) => match p.plan.clone() {
                        Plan::Merge { .. } => render_merge(&req, &ack(())),
                        Plan::Issue(op) => {
                            drop(r);
                            match c.commit(WRITER, *op) {
                                Ok(out) => {
                                    let a = ack(out);
                                    render(&req, &p, &a, &c.repo(0, &host, &dir))
                                }
                                Err(e) => refused(&p, &e),
                            }
                        }
                    },
                },
            };
            (200, reply)
        } else if let Some(rest) = path.strip_prefix("/api/v3/repos/o/r/commits/") {
            let (reference, q) = rest.split_once("/statuses").unwrap_or((rest, ""));
            let reply =
                list_commit_statuses(&c.repo(0, &host, &dir), reference, Page::parse(q.trim_start_matches('?')));
            (reply.status, reply.body)
        } else if path.starts_with("/api/v3/meta") {
            (200, json!({ "installed_version": "3.22.0", "verifiable_password_authentication": false }))
        } else if method == "DELETE" {
            (204, Value::Null)
        } else {
            (404, json!({ "message": "Not Found", "status": "404" }))
        };
        let data = if out.is_null() { Vec::new() } else { serde_json::to_vec(&out).unwrap() };
        write!(
            s,
            "HTTP/1.1 {status} X\r\ncontent-type: application/json; charset=utf-8\r\nx-github-enterprise-version: 3.22.0\r\nx-oauth-scopes: repo, read:org\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            data.len()
        )
        .unwrap();
        s.write_all(&data).unwrap();
    }
}
