//! AM3-WP4's session log as the controller host builds and pushes it (`crates/git/src/session_log.rs`): secrets
//! redacted from every text field, steps and the whole log cut to their caps, the host's own `id` and `pushes`, the
//! one-commit pack git itself indexes and reads back, and the host's answer to the push.

use std::io::Write;

use graftyard_git::init::Signature;
use graftyard_git::pkt::{self, Band};
use graftyard_git::refs::ObjectId;
use graftyard_git::session_log::{
    Harness, LOG_MAX, LOG_PATH, LogReport, OUTPUT_MAX, Pushed, REDACTED, SCHEMA, SessionLog, Step, StepKind, build,
    commit, encode, outcome, push_request, redact, session_ref,
};
use graftyard_identity::SessionId;
use graftyard_types::Timestamp;
use serde_json::json;

const RUN: u128 = 0x0e6a_1b2c_0000_4000_8000_0000_0000_0001;

fn id() -> SessionId {
    SessionId::Run(RUN)
}

fn step(input: &str, output: &str) -> Step {
    Step {
        at: 1,
        kind: StepKind::Tool,
        tool: Some("bash".into()),
        input: input.into(),
        output: output.into(),
        truncated: false,
    }
}

fn report(steps: Vec<Step>) -> LogReport {
    LogReport {
        issue_snapshot: None,
        plan: "fix it".into(),
        steps,
        prs: vec![],
        harness: Harness { name: "claude-code".into(), version: "2.1".into(), model: "m".into() },
    }
}

/// Each secret shape is replaced where it starts a word, including a Graftyard `ghs_` with dots, a PEM key with no END
/// line, and a token the box minted that has no known shape; text that only looks near one stays.
#[test]
fn secrets_are_redacted_and_their_look_alikes_kept() {
    let minted = "v1.0123456789abcdefSECRET";
    let cases = [
        ("token ghp_0123456789abcdefABCDEF0123 ok", "token [redacted] ok"),
        ("ghs_1234_aGVsbG8.d29ybGQ.c2lnbmF0dXJl\n", "[redacted]\n"),
        ("x=github_pat_11ABCDEFG0123456789_abcdefghijklmnop;", "x=[redacted];"),
        ("AKIAABCDEFGHIJKLMNOP is a key", "[redacted] is a key"),
        ("export KEY=sk-ant-api03-abcdefghijklmnopqrstuvwxyz", "export KEY=[redacted]"),
        ("xoxb-1234-5678-abcdefghijklmnop", "[redacted]"),
        ("Authorization: Bearer eyJhbGciOi.payload.sig", "Authorization: [redacted]"),
        ("a\n-----BEGIN RSA PRIVATE KEY-----\nMIIE\n-----END RSA PRIVATE KEY-----\nb", "a\n[redacted]\nb"),
        ("-----BEGIN OPENSSH PRIVATE KEY-----\nb3Bl", "[redacted]"),
        (&format!("auth {minted} done"), "auth [redacted] done"),
        // Kept: a prefix inside a word, one too short to be a token, and a public key block.
        ("mysk-abcdefghijklmnopqrstuvwxyz", "mysk-abcdefghijklmnopqrstuvwxyz"),
        ("ghp_short", "ghp_short"),
        ("-----BEGIN PUBLIC KEY-----\nMIIB", "-----BEGIN PUBLIC KEY-----\nMIIB"),
        ("risk-free AKIA", "risk-free AKIA"),
    ];
    for (input, want) in cases {
        assert_eq!(redact(input, &[minted]), want, "{input:?}");
    }
}

/// `build` takes the host's id and acked heads whatever the harness sent, redacts every text field (steps, the plan,
/// the tool, the harness, the issue snapshot), cuts a step's text to OUTPUT_MAX on a char boundary with `truncated`,
/// and brings a log over LOG_MAX under it.
#[test]
fn the_host_builds_a_redacted_log_within_its_caps() {
    let secret = "ghp_0123456789abcdefABCDEF0123";
    let big = "é".repeat(OUTPUT_MAX); // 2 bytes a char: twice OUTPUT_MAX
    let mut r = report(vec![step(&format!("echo {secret}"), &big), step("ls", "a b")]);
    r.plan = format!("use {secret}");
    r.harness.model = secret.into();
    r.steps[1].tool = Some(secret.into());
    r.issue_snapshot = Some(json!({ "visible": format!("token {secret}"), "hidden": { "html_comments": 0 } }));
    let pushes = [ObjectId([1; 20]), ObjectId([2; 20])];
    let log = build(id(), &pushes, r, &[]);
    let text = String::from_utf8(encode(&log)).unwrap();
    assert!(!text.contains(secret), "no field keeps the secret");
    assert_eq!(text.matches(REDACTED).count(), 5, "input, plan, model, tool, snapshot");
    assert_eq!((log.schema, &log.id), (SCHEMA, &id()));
    assert_eq!(log.pushes, [ObjectId([1; 20]).hex(), ObjectId([2; 20]).hex()]);
    assert!(log.steps[0].output.len() <= OUTPUT_MAX && log.steps[0].output.len() > OUTPUT_MAX - 2);
    assert!(log.steps[0].truncated && !log.steps[1].truncated);
    // Twenty steps at OUTPUT_MAX each are over LOG_MAX: the log is cut to it, and every step that lost text says so.
    let x = "x".repeat(OUTPUT_MAX);
    let r = report((0..20).map(|_| step(&x, &x)).collect());
    let log = build(id(), &[], r, &[]);
    let len = encode(&log).len();
    assert!(len <= LOG_MAX, "{len}");
    assert!(len > LOG_MAX / 2, "cut no more than it must: {len}");
    assert_eq!(log.steps.len(), 20, "halving fields was enough: no step dropped");
    assert!(log.steps.iter().all(|s| s.truncated || s.input.len() + s.output.len() == 2 * OUTPUT_MAX));
    // Many small steps cannot be halved under LOG_MAX: the later ones go, and the last kept says so.
    let r = report((0..40_000).map(|_| step("i", "o")).collect());
    let log = build(id(), &[], r, &[]);
    assert!(encode(&log).len() <= LOG_MAX);
    assert!(log.steps.len() < 40_000 && log.steps.last().unwrap().truncated);
}

fn git(dir: &std::path::Path, args: &[&str], input: Option<&[u8]>) -> String {
    let mut cmd = std::process::Command::new("git");
    cmd.arg("-C").arg(dir).args(args);
    cmd.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped());
    let mut p = cmd.spawn().expect("git runs");
    if let Some(i) = input {
        p.stdin.take().unwrap().write_all(i).unwrap();
    }
    let o = p.wait_with_output().unwrap();
    assert!(o.status.success(), "git {args:?} failed");
    String::from_utf8(o.stdout).unwrap()
}

/// git indexes the pack strictly, and the session ref's tree holds `session.json` alone, which reads back as the log.
#[test]
fn git_reads_the_session_log_commit() {
    let log = build(id(), &[ObjectId([3; 20])], report(vec![step("a", "b")]), &[]);
    let who =
        Signature { name: "graftyard-workspace[bot]", email: "w@users.noreply.h", at: Timestamp(1_793_000_000_000) };
    let (c, pack) = commit(&encode(&log), &id(), &who);
    let dir = std::env::temp_dir().join(format!("graftyard-session-log-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q", "--bare"], None);
    git(&dir, &["index-pack", "--strict", "--stdin"], Some(&pack));
    let hex = c.hex();
    git(&dir, &["fsck", "--strict", "--no-dangling", &hex], None);
    assert_eq!(git(&dir, &["ls-tree", "--name-only", &hex], None), format!("{LOG_PATH}\n"));
    let back: SessionLog = serde_json::from_str(&git(&dir, &["show", &format!("{hex}:{LOG_PATH}")], None)).unwrap();
    assert_eq!(back, log);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The request creates the session ref from zero, and the host's answer reads as git's report-status says: `ok`,
/// plain or side-band, is a push; `ng` or a bad unpack, or a 4xx, is a refusal; a 5xx, no answer, or a 200 that says
/// nothing of the ref is tried again.
#[test]
fn the_push_creates_the_session_ref_and_its_answer_is_read() {
    let (c, pack) = (ObjectId([9; 20]), b"PACKxyz".to_vec());
    let req = push_request(&id(), c, &pack);
    let parsed = graftyard_git::smart::parse_receive_pack(&req).unwrap();
    assert_eq!(parsed.updates.len(), 1);
    assert_eq!(parsed.updates[0].name.display(), session_ref(&id()));
    assert_eq!((parsed.updates[0].expect, parsed.updates[0].new), (None, Some(c)));
    assert_eq!(&req[parsed.pack_at..], &pack[..]);
    let lines = |ls: &[&str]| {
        let mut b = Vec::new();
        ls.iter().for_each(|l| pkt::line(&mut b, l));
        pkt::flush(&mut b);
        b
    };
    let r = session_ref(&id());
    let ok = lines(&["unpack ok", &format!("ok {r}")]);
    assert_eq!(outcome(&id(), &req, Some(200), &ok), Pushed::Ok);
    let mut banded = Vec::new();
    pkt::sideband(&mut banded, Band::Large, 1, &ok);
    pkt::flush(&mut banded);
    assert_eq!(outcome(&id(), &req, Some(200), &banded), Pushed::Ok, "side-band");
    let ng = lines(&["unpack ok", &format!("ng {r} refused: no live task")]);
    assert_eq!(outcome(&id(), &req, Some(200), &ng), Pushed::Refused);
    assert_eq!(outcome(&id(), &req, Some(200), &lines(&["unpack index-pack failed"])), Pushed::Refused);
    assert_eq!(outcome(&id(), &req, Some(401), b""), Pushed::Refused);
    assert_eq!(outcome(&id(), &req, Some(503), &ok), Pushed::Retry);
    assert_eq!(outcome(&id(), &req, None, b""), Pushed::Retry);
    assert_eq!(outcome(&id(), &req, Some(200), b"<html>"), Pushed::Retry);
    // Another task's `ok` is not this one's.
    let other = lines(&["unpack ok", &format!("ok {}", session_ref(&SessionId::Run(RUN + 1)))]);
    assert_ne!(outcome(&id(), &req, Some(200), &other), Pushed::Ok);
}
