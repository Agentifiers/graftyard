//! A Graftyard-run task's session log (`design/AGENT-LAYER.md`, Shape, `SessionLog`; `build/BUILD-PLAN.md`, AM3-WP4),
//! as the controller host builds and pushes it. The harness reports its steps once (`LogReport`, `POST
//! https://controller.graftyard.internal/log`); the host adds what only it knows (the task's id and the heads it saw
//! acked), replaces every secret with `REDACTED` (`redact`), cuts each step's text to `OUTPUT_MAX` and the whole log
//! to `LOG_MAX` (`build`), and pushes the JSON as `LOG_PATH`, alone in a one-commit tree (`commit`), to
//! `refs/graftyard/sessions/<id>` (`push_request`) with the `graftyard-workspace` token. `outcome` reads the host's
//! answer. Reads go through the git cache's `/ui/blob`, whose 1 MiB cap is `LOG_MAX`.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha1::{Digest, Sha1};

use crate::agent::{RefPattern, SESSION_REFS};
use crate::init::{Signature, entry, object_id};
use crate::issues::IssueNumber;
use crate::pkt;
use crate::refs::ObjectId;
use graftyard_identity::SessionId;

/// `SessionLog.schema`.
pub const SCHEMA: u16 = 1;
/// The log's one file in its commit's tree.
pub const LOG_PATH: &str = "session.json";
/// Each step's `input` and `output`, and the plan, at most.
pub const OUTPUT_MAX: usize = 64 << 10;
/// The whole log's JSON at most: the git cache's blob read cap (`containers/git/server/ui.go`, `blobMaxBytes`).
pub const LOG_MAX: usize = 1 << 20;
/// What a secret becomes.
pub const REDACTED: &str = "[redacted]";
/// Push tries with no answer before the box gives the log up [guess]: one in the report, then one per alarm.
pub const LOG_TRIES: u8 = 5;
/// The harness's name, version, and model, each at most this.
const HARNESS_MAX: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    Model,
    Tool,
    Test,
    Push,
}

/// One model turn, tool call, test run, or push, at `at` (ms).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    pub at: u64,
    pub kind: StepKind,
    #[serde(default)]
    pub tool: Option<String>,
    #[serde(default)]
    pub input: String,
    #[serde(default)]
    pub output: String,
    #[serde(default)]
    pub truncated: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Harness {
    pub name: String,
    pub version: String,
    pub model: String,
}

/// What the harness reports: everything in the log but what the host sets.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct LogReport {
    /// The issue's `graftyard_trust` object as the harness read it under the agent media type; none for a prompt-only
    /// task.
    #[serde(default)]
    pub issue_snapshot: Option<Value>,
    #[serde(default)]
    pub plan: String,
    #[serde(default)]
    pub steps: Vec<Step>,
    #[serde(default)]
    pub prs: Vec<IssueNumber>,
    pub harness: Harness,
}

/// `SessionLog` schema 1, as stored and served.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionLog {
    pub schema: u16,
    pub id: SessionId,
    pub issue_snapshot: Option<Value>,
    pub plan: String,
    pub steps: Vec<Step>,
    /// Hex, as git names them.
    pub pushes: Vec<String>,
    pub prs: Vec<IssueNumber>,
    pub harness: Harness,
}

/// The task's session ref.
pub fn session_ref(id: &SessionId) -> String {
    format!("{SESSION_REFS}{}", id.text())
}

// ---- redaction ----

fn word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// How many bytes from `s` are in `ok`.
fn run(s: &[u8], ok: impl Fn(u8) -> bool) -> usize {
    s.iter().take_while(|b| ok(**b)).count()
}

/// The length of the secret that starts `s`, if one does; `s` starts at a word boundary.
fn secret_at(s: &[u8]) -> Option<usize> {
    let tok = |b: u8| word(b) || b == b'.' || b == b'-';
    let starts = |p: &str| s.len() >= p.len() && s[..p.len()].eq_ignore_ascii_case(p.as_bytes());
    // GitHub's token prefixes; a Graftyard `ghs_` token holds dots (`ghs_<id>_<a>.<b>.<c>`).
    for p in ["ghp_", "gho_", "ghu_", "ghs_", "ghr_"] {
        if s.starts_with(p.as_bytes()) {
            let n = run(&s[p.len()..], tok);
            return (n >= 16).then_some(p.len() + n);
        }
    }
    if s.starts_with(b"github_pat_") {
        let n = run(&s[11..], tok);
        return (n >= 20).then_some(11 + n);
    }
    // AWS access key ids.
    for p in ["AKIA", "ASIA"] {
        if s.starts_with(p.as_bytes()) {
            let n = run(&s[4..], |b| b.is_ascii_uppercase() || b.is_ascii_digit());
            return (n >= 16).then_some(4 + n);
        }
    }
    // Model and other API keys (`sk-...`, `sk-ant-...`, `sk-proj-...`) and Slack tokens.
    for p in ["sk-", "xoxb-", "xoxp-", "xoxa-", "xoxs-", "xapp-"] {
        if s.starts_with(p.as_bytes()) {
            let n = run(&s[p.len()..], tok);
            return (n >= 20).then_some(p.len() + n);
        }
    }
    if starts("bearer ") {
        let n = run(&s[7..], |b| tok(b) || b == b'~' || b == b'+' || b == b'/' || b == b'=');
        return (n >= 8).then_some(7 + n);
    }
    // A PEM private key: through its END line, or to the end of the text when it has none.
    if s.starts_with(b"-----BEGIN ") {
        let line = s.iter().position(|b| *b == b'\n').unwrap_or(s.len());
        if !s[..line].windows(11).any(|w| w == b"PRIVATE KEY") {
            return None;
        }
        let end = s.windows(9).position(|w| w == b"-----END ").map(|i| {
            let after = i + 9;
            after + s[after..].windows(5).position(|w| w == b"-----").map_or(s.len() - after, |j| j + 5)
        });
        return Some(end.unwrap_or(s.len()));
    }
    None
}

/// `text` with every secret replaced by `REDACTED`: each of `known` (the tokens the box minted, the model key) and
/// each match of `secret_at` that starts a word.
pub fn redact(text: &str, known: &[&str]) -> String {
    let mut s = text.to_owned();
    for k in known.iter().filter(|k| k.len() >= 8) {
        if s.contains(*k) {
            s = s.replace(*k, REDACTED);
        }
    }
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let (mut i, mut kept) = (0, 0);
    while i < b.len() {
        if (i == 0 || !word(b[i - 1]))
            && let Some(n) = secret_at(&b[i..])
        {
            out.push_str(&s[kept..i]);
            out.push_str(REDACTED);
            i += n;
            kept = i;
            continue;
        }
        i += 1;
    }
    out.push_str(&s[kept..]);
    out
}

/// `s` cut to at most `max` bytes on a char boundary; whether it was cut.
fn cut(s: &mut String, max: usize) -> bool {
    if s.len() <= max {
        return false;
    }
    let mut at = max;
    while !s.is_char_boundary(at) {
        at -= 1;
    }
    s.truncate(at);
    true
}

fn redact_value(v: &mut Value, known: &[&str]) {
    match v {
        Value::String(s) => *s = redact(s, known),
        Value::Array(a) => a.iter_mut().for_each(|x| redact_value(x, known)),
        Value::Object(o) => o.values_mut().for_each(|x| redact_value(x, known)),
        _ => {}
    }
}

/// The log the host pushes: the report with the host's `id` and `pushes`, every text field redacted, each step's
/// `input` and `output` and the plan cut to `OUTPUT_MAX`, an issue snapshot over `OUTPUT_MAX` dropped, then the
/// largest of those halved until the JSON is at most `LOG_MAX`, and past that the later half of the steps dropped. A
/// step that lost text says `truncated`.
pub fn build(id: SessionId, pushes: &[ObjectId], report: LogReport, known: &[&str]) -> SessionLog {
    let LogReport { mut issue_snapshot, plan, mut steps, prs, harness } = report;
    let mut plan = redact(&plan, known);
    cut(&mut plan, OUTPUT_MAX);
    if let Some(v) = issue_snapshot.as_mut() {
        redact_value(v, known);
    }
    if issue_snapshot.as_ref().is_some_and(|v| v.to_string().len() > OUTPUT_MAX) {
        issue_snapshot = None;
    }
    for s in &mut steps {
        s.tool = s.tool.take().map(|t| {
            let mut t = redact(&t, known);
            cut(&mut t, HARNESS_MAX);
            t
        });
        s.input = redact(&s.input, known);
        s.output = redact(&s.output, known);
        let a = cut(&mut s.input, OUTPUT_MAX);
        let b = cut(&mut s.output, OUTPUT_MAX);
        s.truncated |= a || b;
    }
    let mut harness = Harness {
        name: redact(&harness.name, known),
        version: redact(&harness.version, known),
        model: redact(&harness.model, known),
    };
    for f in [&mut harness.name, &mut harness.version, &mut harness.model] {
        cut(f, HARNESS_MAX);
    }
    let pushes = pushes.iter().map(ObjectId::hex).collect();
    let mut log = SessionLog { schema: SCHEMA, id, issue_snapshot, plan, steps, pushes, prs, harness };
    loop {
        let len = encode(&log).len();
        if len <= LOG_MAX {
            return log;
        }
        // The largest field: the plan (as step `None`), or a step's input or output.
        let mut best: (usize, Option<(usize, bool)>) = (log.plan.len(), None);
        for (i, s) in log.steps.iter().enumerate() {
            for (out, f) in [(false, &s.input), (true, &s.output)] {
                if f.len() > best.0 {
                    best = (f.len(), Some((i, out)));
                }
            }
        }
        // Halving a field this small cannot bring a log over LOG_MAX under it: too many steps.
        if best.0 < 64 {
            if log.steps.is_empty() {
                log.issue_snapshot = None;
                log.plan.clear();
                log.pushes.truncate(log.pushes.len() / 2);
                continue;
            }
            let keep = log.steps.len() / 2;
            log.steps.truncate(keep);
            if let Some(last) = log.steps.last_mut() {
                last.truncated = true;
            }
            continue;
        }
        let to = best.0 / 2;
        match best.1 {
            None => {
                cut(&mut log.plan, to);
            }
            Some((i, out)) => {
                let s = &mut log.steps[i];
                cut(if out { &mut s.output } else { &mut s.input }, to);
                s.truncated = true;
            }
        }
    }
}

/// The log's stored bytes.
pub fn encode(log: &SessionLog) -> Vec<u8> {
    serde_json::to_vec(log).expect("a session log serializes")
}

// ---- the commit and the push ----

/// The log's commit and the version 2 pack of its three objects: a root commit whose tree holds `LOG_PATH`.
pub fn commit(json: &[u8], id: &SessionId, who: &Signature<'_>) -> (ObjectId, Vec<u8>) {
    let blob_id = object_id("blob", json);
    let mut tree = format!("100644 {LOG_PATH}\0").into_bytes();
    tree.extend_from_slice(&blob_id.0);
    let tree_id = object_id("tree", &tree);
    let sig = format!("{} <{}> {} +0000", who.name, who.email, who.at.0 / 1000);
    let msg = format!("Session log {}\n", id.text());
    let commit = format!("tree {}\nauthor {sig}\ncommitter {sig}\n\n{msg}", tree_id.hex()).into_bytes();
    let commit_id = object_id("commit", &commit);
    let mut pack = b"PACK".to_vec();
    pack.extend_from_slice(&2u32.to_be_bytes());
    pack.extend_from_slice(&3u32.to_be_bytes());
    entry(&mut pack, 1, &commit);
    entry(&mut pack, 2, &tree);
    entry(&mut pack, 3, json);
    let trailer: [u8; 20] = Sha1::digest(&pack).into();
    pack.extend_from_slice(&trailer);
    (commit_id, pack)
}

/// The `git-receive-pack` request body that creates the session ref at `new` with `pack`.
pub fn push_request(id: &SessionId, new: ObjectId, pack: &[u8]) -> Vec<u8> {
    let mut b = Vec::new();
    let cmd =
        format!("{} {} {}\0report-status agent=graft-controller", ObjectId::ZERO.hex(), new.hex(), session_ref(id));
    pkt::data(&mut b, cmd.as_bytes());
    pkt::flush(&mut b);
    b.extend_from_slice(pack);
    b
}

/// How the host answered a log push.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Pushed {
    /// It took the ref.
    Ok,
    /// It refused for good: a 4xx, or a report-status that refused the ref, as for a task the base already reads
    /// final or a ref already there.
    Refused,
    /// No answer, a 5xx, or one that does not parse: try again.
    Retry,
}

/// The answer to `request` (from `push_request`) with HTTP `status` and body `response`.
pub fn outcome(id: &SessionId, request: &[u8], status: Option<u16>, response: &[u8]) -> Pushed {
    match status {
        None => return Pushed::Retry,
        Some(s) if s >= 500 => return Pushed::Retry,
        Some(s) if !(200..300).contains(&s) => return Pushed::Refused,
        _ => {}
    }
    let Ok(pattern) = RefPattern::parse(&session_ref(id)) else { return Pushed::Refused };
    if !crate::taskbox::acked_pushes(request, response, &pattern).is_empty() {
        return Pushed::Ok;
    }
    // A 200 that names the ref `ng`, or unpacks badly, refused it; one that says nothing of it did not answer.
    let text = String::from_utf8_lossy(response);
    if text.contains(&format!("ng {}", session_ref(id))) || text.contains("unpack ") {
        Pushed::Refused
    } else {
        Pushed::Retry
    }
}
