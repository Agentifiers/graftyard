//! Agent actors and refusals in `decide` (`design/DESIGN.md`, Shape, "Agent load and agent safety";
//! `design/AGENT-LAYER.md`, Shape, "Agent actors in the layer"; `build/BUILD-PLAN.md`, AM1-WP2).
//!
//! Everything here is pure. `classify` turns a principal and the install's `Authz(Genesis.account)` fold into an
//! `ActorKind` and an `AdmitClass`. The trailer parser, `changed_of`, and `protected_of` are `validate`'s rules over
//! what the git cache reports, ported from the M0-WP10 prototype (`tests/m0/src/agent/`), which B0-36 and B0-37 ran.
//! The refusal texts name the allowed prefix or the protected path, because agents recover by reading them.

use graftyard_identity::glob::glob_match;
use graftyard_identity::{Actor, AgentApp, AgentKind, AppId, AuthzRead, Principal, UserId, install_account};
use graftyard_raft::admit::AdmitClass;
use graftyard_types::{ChainId, RepoId};
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

use crate::issues::IssueNumber;
use crate::refs::{ObjectId, RefName};

pub use graftyard_identity::glob::{PathGlob, RefPattern};

/// Host-owned: `decide` refuses every update under it for every actor (`SESSION_REFS`).
pub const HOST_REFS: &str = "refs/graftyard/";
/// Session logs, under `HOST_REFS`. Only `graftyard-workspace` with a live task's `task` claim may append one, and no
/// task exists before AM1-WP3, so every update is refused.
pub const SESSION_REFS: &str = "refs/graftyard/sessions/";
/// git-ai's notes ref. An `Agent` update of it is outside its prefix.
pub const NOTES_AI: &str = "refs/notes/ai";

/// Plus the install's harness template paths and the paths a repository's agent policy adds.
pub const PROTECTED_PATHS: &[&str] = &[
    ".github/workflows/**",
    ".github/actions/**",
    ".gitmodules",
    "CODEOWNERS",
    ".github/CODEOWNERS",
    "docs/CODEOWNERS",
    "**/AGENTS.md",
    "**/CLAUDE.md",
    "**/GEMINI.md",
    ".github/agents/**",
    ".github/copilot-instructions.md",
    ".github/instructions/**",
    ".claude/**",
    ".cursor/**",
    ".cursorrules",
    ".codex/**",
    ".mcp.json",
    ".vscode/**",
];
pub const PATHS_MAX: usize = 1_000;
/// At most this many recognized trailers kept per commit, and bytes per value [guess, the M0-WP10 prototype's caps].
pub const TRAILERS_MAX: usize = 32;
pub const VALUE_MAX: usize = 1_024;
/// Only the last MiB of a message is scanned for its trailer block [guess, the prototype's cap].
pub const MESSAGE_SCAN_MAX: usize = 1 << 20;

/// A task's id: AM1-WP3 builds the tasks, so here it is only the shape `AgentActor`, `AgentMeta`, and
/// `Source::Session` carry. Its string form is the lowercase hyphenated UUID, or `<app_id>.<name>`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum SessionId {
    Run(u128),
    External { app: AppId, name: Box<str> },
}

impl SessionId {
    /// A session's string form. A bare `name` is scoped to `caller`, as the host scopes an App's own name.
    pub fn parse(s: &str, caller: Option<AppId>) -> Option<SessionId> {
        if let Some(u) = parse_uuid(s) {
            return Some(SessionId::Run(u));
        }
        let (app, name) = match s.split_once('.') {
            Some((a, n)) if !a.is_empty() && a.bytes().all(|b| b.is_ascii_digit()) => {
                (AppId(graftyard_types::DbId(a.parse().ok()?)), n)
            }
            _ => (caller?, s),
        };
        let ok = (1..=64).contains(&name.len())
            && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
            && name.as_bytes()[0].is_ascii_alphanumeric()
            && !name.contains("..")
            && !name.ends_with(".lock");
        ok.then(|| SessionId::External { app, name: name.into() })
    }

    pub fn text(&self) -> String {
        match self {
            SessionId::Run(u) => {
                let h = format!("{u:032x}");
                format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
            }
            SessionId::External { app, name } => format!("{}.{name}", app.0.0),
        }
    }
}

fn parse_uuid(s: &str) -> Option<u128> {
    let b = s.as_bytes();
    let dashes = [8, 13, 18, 23];
    if b.len() != 36 || dashes.iter().any(|i| b[*i] != b'-') {
        return None;
    }
    let hex: String = s.chars().filter(|c| *c != '-').collect();
    if !hex.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)) {
        return None;
    }
    u128::from_str_radix(&hex, 16).ok()
}

/// Which kind of actor `decide` binds. Only `Agent` is refused the agent rules.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActorKind {
    User,
    Bot,
    Agent(AgentActor),
}

/// `task` is the installation token's `task` claim (none before AM1-WP3). `requester` is that task's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentActor {
    pub app: AppId,
    pub task: Option<SessionId>,
    pub requester: Option<UserId>,
}

/// Captured on `PrOpened` when the creating token belongs to an `AgentApp`. A PR with one merges only through
/// `AgentGate`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentMeta {
    pub app: AppId,
    pub session: Option<SessionId>,
    pub requester: Option<UserId>,
    pub repo: RepoId,
}

/// One qualifying APPROVE of `pr` at `head`. `Merge` counts distinct `satisfied_by` on the current head.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentGate {
    pub pr: IssueNumber,
    pub head: ObjectId,
    pub satisfied_by: UserId,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CiMode {
    #[default]
    Approve,
    AutoReadOnly,
}

/// `AgentPolicy.leases`. AM1-WP4 builds leases; until then nothing reads it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LeaseEnforce {
    #[default]
    Advisory,
    AgentsExclusive,
}

/// Per repository, a core row. The default when no row exists.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentPolicy {
    pub ci: CiMode,
    /// The labels agents may write.
    pub labels: SmallVec<[Box<str>; 8]>,
    /// Added to `PROTECTED_PATHS`.
    pub extra_paths: Vec<Box<str>>,
    pub extra_human_approvals: u8,
    pub leases: LeaseEnforce,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtectedHits {
    pub refname: RefName,
    pub paths: SmallVec<[Box<str>; 4]>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangedPaths {
    pub refname: RefName,
    pub paths: Vec<Box<str>>,
    pub truncated: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum TrailerKey {
    GraftyardSession,
    ClaudeSession,
    AmpThreadId,
    CoAuthoredBy,
    AssistedBy,
}

impl TrailerKey {
    /// git matches trailer keys case-insensitively.
    pub fn of(token: &str) -> Option<TrailerKey> {
        Some(match token.to_ascii_lowercase().as_str() {
            "graftyard-session" => TrailerKey::GraftyardSession,
            "claude-session" => TrailerKey::ClaudeSession,
            "amp-thread-id" => TrailerKey::AmpThreadId,
            "co-authored-by" => TrailerKey::CoAuthoredBy,
            "assisted-by" => TrailerKey::AssistedBy,
            _ => return None,
        })
    }

    /// Only these class an App token's push. `CoAuthoredBy` is recorded and never classes.
    pub fn classes(self) -> bool {
        self != TrailerKey::CoAuthoredBy
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Source {
    Claimed { key: TrailerKey, value: Box<str> },
    AiNote { note: ObjectId },
    Session(SessionId),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    pub commit: ObjectId,
    pub refname: RefName,
    pub sources: SmallVec<[Source; 2]>,
}

// ---- validate's rules ----

/// Every changed path, or past PATHS_MAX their directories with `truncated` set: the deepest directory level whose
/// distinct prefixes fit, as the prototype cut it.
pub fn changed_of(refname: &RefName, paths: &[&str]) -> ChangedPaths {
    let mut sorted: Vec<&str> = paths.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    if sorted.len() <= PATHS_MAX {
        return ChangedPaths {
            refname: refname.clone(),
            paths: sorted.into_iter().map(Into::into).collect(),
            truncated: false,
        };
    }
    let mut dirs: Vec<String> = Vec::new();
    for depth in (1..=8).rev() {
        dirs = sorted
            .iter()
            .map(|p| {
                let segs: Vec<&str> = p.split('/').collect();
                if segs.len() > depth { format!("{}/", segs[..depth].join("/")) } else { (*p).to_owned() }
            })
            .collect();
        dirs.dedup();
        if dirs.len() <= PATHS_MAX {
            break;
        }
    }
    dirs.truncate(PATHS_MAX);
    ChangedPaths { refname: refname.clone(), paths: dirs.into_iter().map(Into::into).collect(), truncated: true }
}

/// The changed paths that match `PROTECTED_PATHS` or the policy's `extra_paths`, sorted; `None` when none does.
pub fn protected_of(refname: &RefName, paths: &[&str], extra: &[Box<str>]) -> Option<ProtectedHits> {
    let mut hits: Vec<&str> = paths
        .iter()
        .copied()
        .filter(|p| PROTECTED_PATHS.iter().copied().chain(extra.iter().map(|e| &**e)).any(|g| glob_match(g, p)))
        .collect();
    hits.sort_unstable();
    hits.dedup();
    (!hits.is_empty())
        .then(|| ProtectedHits { refname: refname.clone(), paths: hits.into_iter().map(Into::into).collect() })
}

fn strip(c: char) -> bool {
    let u = c as u32;
    (u < 0x20 && c != '\t')
        || (0x7f..=0x9f).contains(&u)
        || (0x200b..=0x200f).contains(&u)
        || (0x202a..=0x202e).contains(&u)
        || (0x2060..=0x2064).contains(&u)
        || (0x2066..=0x2069).contains(&u)
        || u == 0xfeff
}

fn clean(v: &str) -> Box<str> {
    let s: String = v.chars().filter(|c| !strip(*c)).collect();
    let s = s.trim();
    let mut end = s.len().min(VALUE_MAX);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].into()
}

/// The message of a raw commit object: what follows the blank line that ends the header, of which only the last
/// MESSAGE_SCAN_MAX bytes are read. Never fails: invalid UTF-8 is replaced.
pub fn commit_message(raw: &[u8]) -> String {
    let body = raw.windows(2).position(|w| w == b"\n\n").map_or(&raw[raw.len()..], |i| &raw[i + 2..]);
    let tail = &body[body.len().saturating_sub(MESSAGE_SCAN_MAX)..];
    String::from_utf8_lossy(tail).into_owned()
}

/// git's trailer block, as `git interpret-trailers --parse` reads it, simplified as the prototype did: the last
/// paragraph, after at least one other, where every line is `Token: value` or a continuation, or at least a quarter
/// are trailers and one is a recognized key. Returns the recognized trailers only, at most TRAILERS_MAX.
pub fn parse_trailers(message: &str) -> Vec<(TrailerKey, Box<str>)> {
    let m = message.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = m.trim_end_matches('\n').split('\n').collect();
    let mut start = lines.len();
    while start > 0 && !lines[start - 1].trim().is_empty() {
        start -= 1;
    }
    if start == 0 {
        return Vec::new();
    }
    let mut items: Vec<(String, String)> = Vec::new();
    let (mut trailers, mut other, mut recognized) = (0usize, 0usize, false);
    for line in &lines[start..] {
        if line.starts_with([' ', '\t']) && !items.is_empty() {
            let last = items.last_mut().expect("an item");
            last.1.push(' ');
            last.1.push_str(line.trim());
            continue;
        }
        match token_line(line) {
            Some((t, v)) => {
                trailers += 1;
                recognized |= TrailerKey::of(t).is_some();
                items.push((t.to_owned(), v.to_owned()));
            }
            None => other += 1,
        }
    }
    if trailers == 0 || (other > 0 && !(recognized && trailers * 4 >= trailers + other)) {
        return Vec::new();
    }
    items.iter().filter_map(|(t, v)| Some((TrailerKey::of(t)?, clean(v)))).take(TRAILERS_MAX).collect()
}

/// `Token: value`, where the token is ASCII alphanumerics and `-`, starting with an alphanumeric.
fn token_line(line: &str) -> Option<(&str, &str)> {
    let (t, v) = line.split_once(':')?;
    let t = t.trim_end_matches([' ', '\t']);
    let ok = t.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
        && t.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
    ok.then(|| (t, v.trim_start_matches([' ', '\t'])))
}

/// One changed entry of the `refs/notes/ai` tree: its fan-out path and blob.
pub struct NoteEntry<'a> {
    pub path: &'a str,
    pub blob: bool,
    pub oid: ObjectId,
}

/// `validate`'s `Provenance`: each new commit's recognized trailers as `Claimed`, and an `AiNote` for each note blob
/// whose fan-out path names a commit. It never fails: a commit whose message yields nothing contributes nothing, and
/// a malformed note entry is skipped.
pub fn provenance_of(refname: &RefName, commits: &[(ObjectId, &[u8])], notes: &[NoteEntry<'_>]) -> Vec<Provenance> {
    let mut out: Vec<Provenance> = Vec::new();
    for (oid, raw) in commits {
        let t = parse_trailers(&commit_message(raw));
        if !t.is_empty() {
            let sources = t.into_iter().map(|(key, value)| Source::Claimed { key, value }).collect();
            out.push(Provenance { commit: *oid, refname: refname.clone(), sources });
        }
    }
    for n in notes {
        let target: String = n.path.chars().filter(|c| *c != '/').collect();
        let Some(commit) = ObjectId::parse_hex(target.to_ascii_lowercase().as_bytes()).filter(|_| n.blob) else {
            continue;
        };
        match out.iter_mut().find(|p| p.commit == commit) {
            Some(p) => p.sources.push(Source::AiNote { note: n.oid }),
            None => out.push(Provenance {
                commit,
                refname: refname.clone(),
                sources: smallvec::smallvec![Source::AiNote { note: n.oid }],
            }),
        }
    }
    out
}

/// Whether any source would class an App token's push as Agent.
pub fn classes(list: &[Provenance]) -> bool {
    list.iter().any(|p| p.sources.iter().any(|s| matches!(s, Source::Claimed { key, .. } if key.classes())))
}

/// The session a `Graftyard-Session` value names: exactly `<origin>/_graftyard/sessions/<id>`, where `origin` is
/// `https://<host>/<base owner>/<base repo>` as the edge that took the push wrote it. Anything else is `None`.
pub fn session_from_url(value: &str, origin: &str, app: AppId) -> Option<SessionId> {
    let lower = |s: &str| s.to_ascii_lowercase();
    if !lower(origin).starts_with("https://") || value.len() <= origin.len() {
        return None;
    }
    let (head, rest) = value.split_at(origin.len());
    if lower(head) != lower(origin) {
        return None;
    }
    let id = rest.strip_prefix("/_graftyard/sessions/")?;
    if id.is_empty() || id.contains(['/', '?', '#', '%']) {
        return None;
    }
    SessionId::parse(id, Some(app))
}

/// `decide`'s linking rule: a `Graftyard-Session` trailer becomes `Session` only when the pusher is an App's
/// installation token and the value names one of `own`, its App's sessions. Every other trailer stays `Claimed`.
pub fn link(list: &[Provenance], pusher: Option<AppId>, own: &[SessionId], origin: Option<&str>) -> Vec<Provenance> {
    list.iter()
        .map(|p| {
            let sources = p
                .sources
                .iter()
                .map(|s| match (s, pusher, origin) {
                    (Source::Claimed { key: TrailerKey::GraftyardSession, value }, Some(app), Some(o)) => {
                        match session_from_url(value, o, app) {
                            Some(id) if own.contains(&id) => Source::Session(id),
                            _ => s.clone(),
                        }
                    }
                    _ => s.clone(),
                })
                .collect();
            Provenance { commit: p.commit, refname: p.refname.clone(), sources }
        })
        .collect()
}

/// What `decide` stores of a push's provenance: a `Session` source only when it names one of `own`, the pusher's
/// App's sessions (none before AM1-WP3), so a `Session` sent on the wire without a task links nothing. Every other
/// source is kept as the edge's `validate` gave it.
pub fn unlinked(list: &[Provenance], own: &[SessionId]) -> Vec<Provenance> {
    list.iter()
        .map(|p| {
            let sources = p
                .sources
                .iter()
                .filter(|s| !matches!(s, Source::Session(id) if !own.contains(id)))
                .cloned()
                .collect::<SmallVec<[Source; 2]>>();
            Provenance { commit: p.commit, refname: p.refname.clone(), sources }
        })
        .filter(|p| !p.sources.is_empty())
        .collect()
}

// ---- classing ----

/// The install's own `Authz` fold among a command's deps, the chain that holds `AgentApp` rows.
pub fn install_fold<'a, D>(
    deps: &'a [(ChainId, D)],
    f: impl Fn(&'a D) -> Option<&'a dyn AuthzRead>,
) -> Option<&'a dyn AuthzRead> {
    let install = install_account();
    deps.iter().find_map(|(c, d)| match c {
        ChainId::Authz(o) if *o == install => f(d).filter(|a| a.owner() == Some(install)),
        _ => None,
    })
}

/// The `install`'s `AgentApp` row of `app`.
pub fn agent_app(install: Option<&dyn AuthzRead>, app: AppId) -> Option<AgentApp> {
    install.and_then(|a| a.agent_app(app).cloned())
}

/// What a principal is, from the install's fresh `AgentApp` rows: an App is `Agent` when it is a `GraftyardRun`
/// App, or a flagged App with `confine`; a flagged App without `confine` is `Bot`. User tokens and `Graftyard MCP`
/// are never `Agent`.
pub fn actor_kind(p: &Principal, install: Option<&dyn AuthzRead>) -> ActorKind {
    match p.actor {
        Actor::App { app, .. } => match agent_app(install, app) {
            Some(AgentApp { kind: AgentKind::GraftyardRun, .. })
            | Some(AgentApp { kind: AgentKind::ThirdParty, confine: Some(_), .. }) => {
                ActorKind::Agent(AgentActor { app, task: None, requester: None })
            }
            _ => ActorKind::Bot,
        },
        Actor::User(_) | Actor::Anonymous => ActorKind::User,
    }
}

/// The admission class (AM1-WP1's `AdmitClass`): Agent when the actor is an `AgentApp` (`Graftyard MCP` included),
/// when the token is an `AgentApp`'s user-to-server token, when an App token's push carries a classing trailer, or
/// when a user token sends `X-Graftyard-Agent-Session`. A trailer never classes a user-token push on its own.
pub fn admit_class(
    p: &Principal,
    install: Option<&dyn AuthzRead>,
    token_app: Option<AppId>,
    provenance: &[Provenance],
    session_header: bool,
) -> AdmitClass {
    match p.actor {
        Actor::App { app, .. } => {
            if agent_app(install, app).is_some() || classes(provenance) {
                AdmitClass::Agent
            } else {
                AdmitClass::Bot
            }
        }
        Actor::User(_) | Actor::Anonymous => {
            let flagged = token_app.is_some_and(|a| agent_app(install, a).is_some());
            if flagged || session_header { AdmitClass::Agent } else { AdmitClass::Human }
        }
    }
}

// ---- refusals ----

/// Which agent rule refused a command. The order of the push rules is the design's: host-owned ref, stale or missing
/// object, tag creation, live task, prefix, ruleset bypass, protected hit, lease.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AgentRule {
    Tag,
    NoLiveTask,
    OutsidePrefix,
    Bypass,
    Protected,
    Approve,
    Merge,
    ReadyForReview,
    Rules,
    Label,
    GraftyardContext,
    Release,
    DraftOnly,
    /// A `Merge` of a PR with `AgentMeta` before `AgentGate` holds on its head.
    Gate,
    /// An App's command reached `decide` without the `validate` outputs an agent push needs, or classed as another
    /// kind by an edge whose read is now stale: the edge reads the install's rows again and re-sends.
    Reclass,
}

/// A refusal and its text. The text names the rule, the allowed prefix, or the protected path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRefusal {
    pub rule: AgentRule,
    pub text: Box<str>,
}

impl AgentRefusal {
    pub fn new(rule: AgentRule, text: impl Into<Box<str>>) -> AgentRefusal {
        AgentRefusal { rule, text: text.into() }
    }

    pub fn of(rule: AgentRule) -> AgentRefusal {
        let text = match rule {
            AgentRule::Tag | AgentRule::Release => "refused: agents may not create releases or tags",
            AgentRule::Approve => "refused: agents may not approve pull requests",
            AgentRule::Merge => "refused: agents may not merge pull requests",
            AgentRule::ReadyForReview => "refused: agents may not mark pull requests ready for review",
            AgentRule::Rules => "refused: agents may not edit rulesets, branch protection, settings, or bypass lists",
            AgentRule::DraftOnly => "refused: Graftyard-run agents open draft pull requests only",
            AgentRule::Reclass => "refused: the agent class changed; retry",
            AgentRule::NoLiveTask => "refused: push needs a live task; the token names no task",
            AgentRule::OutsidePrefix
            | AgentRule::Bypass
            | AgentRule::Protected
            | AgentRule::Label
            | AgentRule::GraftyardContext
            | AgentRule::Gate => "refused",
        };
        AgentRefusal::new(rule, text)
    }
}

/// The allowed prefix of an `Agent`: its `AgentApp`'s `confine`. A Graftyard-run App's prefix is its task's, and no
/// task exists before AM1-WP3.
pub fn allowed_prefix(install: Option<&dyn AuthzRead>, a: &AgentActor) -> Option<RefPattern> {
    agent_app(install, a.app).and_then(|r| r.confine)
}

/// The agent rules a ref update breaks, in order, for an `Agent` whose update is not host-owned and not stale.
/// `rule_over` says whether a branch rule would refuse the update without a bypass.
pub fn refuse_update(
    install: Option<&dyn AuthzRead>,
    a: &AgentActor,
    name: &RefName,
    new: Option<ObjectId>,
    protected: &[ProtectedHits],
    needs_bypass: bool,
) -> Option<AgentRefusal> {
    if name.starts_with("refs/tags/") && new.is_some() {
        return Some(AgentRefusal::of(AgentRule::Tag));
    }
    let row = agent_app(install, a.app);
    if row.as_ref().is_some_and(|r| r.kind == AgentKind::GraftyardRun) && a.task.is_none() {
        return Some(AgentRefusal::of(AgentRule::NoLiveTask));
    }
    let prefix = allowed_prefix(install, a);
    if prefix.as_ref().is_none_or(|p| !p.matches(name.as_bytes())) {
        let who = format!("app {}", a.app.0.0);
        let p = prefix.as_ref().map_or("(no prefix)", |p| p.as_str());
        return Some(AgentRefusal::new(AgentRule::OutsidePrefix, format!("refused: {who} may push only {p}")));
    }
    if needs_bypass {
        let text = format!("refused: agents may not use a ruleset bypass ({})", name.display());
        return Some(AgentRefusal::new(AgentRule::Bypass, text));
    }
    if new.is_some()
        && let Some(h) = protected.iter().find(|h| h.refname == *name && !h.paths.is_empty())
    {
        let text = format!("refused: {} is a protected path; agents may not change it", h.paths[0]);
        return Some(AgentRefusal::new(AgentRule::Protected, text));
    }
    None
}

/// Approvals `AgentGate` needs: one, plus `extra_human_approvals`, plus one more when `requester` is empty, because
/// the requester cannot be excluded.
pub fn approvals_needed(meta: &AgentMeta, policy: &AgentPolicy) -> usize {
    1 + usize::from(policy.extra_human_approvals) + usize::from(meta.requester.is_none())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(s: &str) -> RefName {
        RefName::parse(s.as_bytes()).unwrap()
    }

    #[test]
    fn trailers_follow_the_last_paragraph_and_strip_hidden_characters() {
        let m =
            "Fix it\n\nBody text.\n\nCo-authored-by: A <a@x>\nclaude-session: abc\u{202e}d\nSigned-off-by: B <b@x>\n";
        let t = parse_trailers(m);
        assert_eq!(t, vec![(TrailerKey::CoAuthoredBy, "A <a@x>".into()), (TrailerKey::ClaudeSession, "abcd".into())]);
        // A subject alone has no trailer block.
        assert!(parse_trailers("Amp-Thread-ID: x").is_empty());
        // A paragraph under a quarter trailers is not a trailer block; at a quarter with a known key it is.
        assert!(parse_trailers("s\n\nprose\nmore prose\nmore\nand more\nAssisted-by: x").is_empty());
        assert_eq!(parse_trailers("s\n\nprose\nmore\nmore\nAssisted-by: x").len(), 1);
        // Continuations join, and a long value is cut at VALUE_MAX on a character boundary.
        let long = "é".repeat(VALUE_MAX);
        let t = parse_trailers(&format!("s\n\nAssisted-by: a\n b\nAmp-Thread-ID: {long}"));
        assert_eq!(&*t[0].1, "a b");
        assert!(t[1].1.len() <= VALUE_MAX && t[1].1.len() >= VALUE_MAX - 1);
        // At most TRAILERS_MAX are kept.
        let many: String = (0..40).map(|i| format!("Assisted-by: {i}\n")).collect();
        assert_eq!(parse_trailers(&format!("s\n\n{many}")).len(), TRAILERS_MAX);
    }

    #[test]
    fn only_classing_trailers_class_and_a_forged_session_stays_claimed() {
        let r = name("refs/heads/claude/a");
        let raw = b"tree 0\nauthor x\n\nfix\n\nCo-authored-by: Claude <noreply@anthropic.com>\n";
        let p = provenance_of(&r, &[(ObjectId([1; 20]), raw)], &[]);
        assert!(!classes(&p), "Co-authored-by alone never classes");
        let raw = b"tree 0\n\nfix\n\nGraftyard-Session: https://h.example/o/r/_graftyard/sessions/3.mine\n";
        let p = provenance_of(&r, &[(ObjectId([2; 20]), raw)], &[]);
        assert!(classes(&p));
        let app = AppId(graftyard_types::DbId(3));
        let mine = SessionId::parse("mine", Some(app)).unwrap();
        let origin = "https://h.example/o/r";
        // Not the pusher's session, another host, or a user token: Claimed.
        assert_eq!(link(&p, Some(app), &[], Some(origin)), p);
        assert_eq!(link(&p, Some(app), std::slice::from_ref(&mine), Some("https://evil.example/o/r")), p);
        assert_eq!(link(&p, None, std::slice::from_ref(&mine), Some(origin)), p);
        let l = link(&p, Some(app), std::slice::from_ref(&mine), Some(origin));
        assert_eq!(l[0].sources[0], Source::Session(mine));
    }

    #[test]
    fn notes_name_commits_by_fan_out_path_and_malformed_ones_are_skipped() {
        let r = name("refs/notes/ai");
        let hex = "ab".repeat(20);
        let path = format!("{}/{}", &hex[..2], &hex[2..]);
        let notes = [
            NoteEntry { path: &path, blob: true, oid: ObjectId([9; 20]) },
            NoteEntry { path: "not-a-commit", blob: true, oid: ObjectId([8; 20]) },
            NoteEntry { path: &hex, blob: false, oid: ObjectId([7; 20]) },
        ];
        let p = provenance_of(&r, &[(ObjectId([0xab; 20]), b"garbage \xff\xfe")], &notes);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].sources.as_slice(), [Source::AiNote { note: ObjectId([9; 20]) }]);
    }

    #[test]
    fn changed_paths_fold_to_directories_past_paths_max_and_protected_paths_match() {
        let r = name("refs/heads/claude/a");
        let many: Vec<String> = (0..1_500).map(|i| format!("src/m{}/f{i}.rs", i % 10)).collect();
        let refs: Vec<&str> = many.iter().map(String::as_str).collect();
        let c = changed_of(&r, &refs);
        assert!(c.truncated && c.paths.len() <= PATHS_MAX && c.paths.iter().all(|p| p.ends_with('/')));
        let c = changed_of(&r, &["b", "a", "a"]);
        assert_eq!((c.paths.len(), c.truncated), (2, false));
        let h = protected_of(&r, &["src/x.rs", "pkg/AGENTS.md", ".github/workflows/ci.yml"], &[]).unwrap();
        assert_eq!(h.paths.as_slice(), [Box::from(".github/workflows/ci.yml"), Box::from("pkg/AGENTS.md")]);
        assert!(protected_of(&r, &["Makefile"], &[]).is_none());
        assert!(protected_of(&r, &["Makefile"], &["Makefile".into()]).is_some());
    }

    #[test]
    fn session_ids_parse_as_the_host_scopes_them() {
        let app = AppId(graftyard_types::DbId(12));
        let u = "0193a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b";
        assert_eq!(SessionId::parse(u, None).unwrap().text(), u);
        assert_eq!(SessionId::parse("fix-1", Some(app)).unwrap().text(), "12.fix-1");
        assert_eq!(SessionId::parse("12.fix-1", None), SessionId::parse("fix-1", Some(app)));
        for bad in ["", "-x", "a..b", "x.lock", "UPPER", "fix-1"] {
            assert!(SessionId::parse(bad, None).is_none(), "{bad}");
        }
    }
}
