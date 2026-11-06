//! The trust view (`design/AGENT-LAYER.md`, Shape, "Trust view", and journey A4; `build/BUILD-PLAN.md`, AM1-WP5):
//! under `AGENT_MEDIA_TYPE`, each object of a trust resource gets `graftyard_trust` beside its untouched `body`, with
//! the text a reader of GitHub's rendering sees (`visible`), what was dropped (`hidden`), and who wrote it (`author`).
//!
//! The edge calls `layer` on every `/api/v3/repos/{o}/{r}/...` reply. Without the media type it returns the reply it
//! was given, so a client that does not opt in gets the bytes it got before. GraphQL never gets the field.
//!
//! `visible` is a port of the M0-WP13 prototype (`tests/m0/src/agent/trust.ts`), whose 200 L0-09 payloads it
//! reproduces (`tests/trust.rs`). The prototype's rules were set against GitHub's renderer (`POST /markdown`, gfm):
//! - Block context, line by line: a line that starts (after container markers `>`, `-`, `*`, `+`, `1.`) with `<` and a
//!   tag name, `/`, `!`, or `?` opens raw-HTML context until a blank line, even where CommonMark would not, because raw
//!   HTML hides more than markdown does. A fence is code only outside raw-HTML context.
//! - Outside code: HTML comments (HTML5 closers `<!-->`, `<!--->`, `-->`, `--!>`), processing instructions,
//!   declarations, and CDATA are dropped and counted in `html_comments`. In raw-HTML context an unclosed comment runs
//!   to the end of the text; in a paragraph it must close before a blank line or it is literal text.
//! - Raw HTML tags keep their name and `href`/`src` and lose every other attribute. A `title` attribute, any other
//!   dropped attribute, a markdown link or image title, and a reference definition's title count in `link_title_text`.
//! - `<details>` without `open` hides everything but its first `<summary>` and counts once in `collapsed`. An opening
//!   tag counts in code too, and `</details>` only outside code and comments, so the hidden region can only grow.
//! - `HIDDEN_CHARS`, raw anywhere or as a character reference outside code, are dropped and each counts in
//!   `invisible_chars`. In code a reference is literal text and stays.
//! - Reference and footnote definitions nothing refers to render as nothing and are dropped; one without a title
//!   counts in `html_comments`.
//!
//! Where the port drops more than the prototype: hidden characters and references inside kept `href`/`src` values and
//! link destinations, and a reference definition's title on a CRLF line.

use std::ops::RangeInclusive;

use graftyard_git::issues::CheckRunUpdate;
use graftyard_git::tiers::{CommentRow, ReleaseRow, ReviewRow};
use graftyard_identity::RepoPermission;
use graftyard_types::{AccountId, DbId};
use graftyard_wire::{Body, Reply};
use serde::Serialize;
use serde_json::{Map, Value};

use crate::view::{Item, Repo};

pub const AGENT_MEDIA_TYPE: &str = "application/vnd.graftyard.agent+json";

/// Added beside `body` and `body_html`, which stay byte-identical to GHES.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TrustView {
    pub visible: Box<str>,
    pub hidden: Hidden,
    pub author: AuthorTrust,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Hidden {
    pub html_comments: u32,
    pub invisible_chars: u32,
    pub link_title_text: u32,
    pub collapsed: u32,
}

/// What `visible` drops and `invisible_chars` counts: soft hyphen, format and bidirectional controls, Hangul and other
/// fillers, interlinear annotation marks, the tag block, and the variation-selector supplement. Emoji variation
/// selectors U+FE00 to U+FE0F stay, because they change only the glyph [inference].
pub const HIDDEN_CHARS: &[RangeInclusive<char>] = &[
    '\u{00AD}'..='\u{00AD}',
    '\u{034F}'..='\u{034F}',
    '\u{061C}'..='\u{061C}',
    '\u{115F}'..='\u{1160}',
    '\u{17B4}'..='\u{17B5}',
    '\u{180E}'..='\u{180E}',
    '\u{200B}'..='\u{200F}',
    '\u{202A}'..='\u{202E}',
    '\u{2060}'..='\u{2064}',
    '\u{2066}'..='\u{206F}',
    '\u{3164}'..='\u{3164}',
    '\u{FEFF}'..='\u{FEFF}',
    '\u{FFA0}'..='\u{FFA0}',
    '\u{FFF9}'..='\u{FFFB}',
    '\u{E0000}'..='\u{E007F}',
    '\u{E0100}'..='\u{E01EF}',
];

/// `trusted` means a human who can push. Bots and every `AgentApp` are never trusted authors.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct AuthorTrust {
    /// GitHub's `author_association`, from `wire::association`.
    pub association: &'static str,
    pub can_push: bool,
    pub is_bot: bool,
    /// `AgentApp` rows are AM1-WP2's, so no author is an agent yet.
    pub is_agent: bool,
    /// No fold row records a first contribution yet, so this is false.
    pub first_contribution: bool,
    pub trusted: bool,
}

/// The objects that get `graftyard_trust`, and the text each one's view is of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resource {
    Issue,
    PullRequest,
    IssueComment,
    Review,
    ReviewComment,
    Release,
    /// `commit.message`.
    Commit,
    /// `output.title`, `output.summary`, and `output.text`, joined by blank lines.
    CheckRun,
}

/// Whether `Accept` names the agent media type among its ranges.
pub fn opted_in(accept: Option<&str>) -> bool {
    accept.is_some_and(|h| {
        h.split(',').any(|t| t.split(';').next().unwrap_or("").trim().eq_ignore_ascii_case(AGENT_MEDIA_TYPE))
    })
}

/// The resource a `/repos/{o}/{r}/...` REST path's 2xx body holds, from the routes of the pinned `ghes-3.22`
/// description whose response is one of the trust resources or a list of them.
pub fn resource_of(rest_path: &str) -> Option<Resource> {
    let rest = rest_path.strip_prefix("/repos/")?;
    let segs: Vec<&str> = rest.split('/').skip(2).collect();
    let num = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    use Resource as R;
    Some(match segs.as_slice() {
        ["issues"] => R::Issue,
        ["issues", "comments"] | ["issues", "comments", _] | ["issues", "comments", _, "pin"] => R::IssueComment,
        ["issues", n, "comments"] if num(n) => R::IssueComment,
        ["issues", n] | ["issues", n, "assignees"] if num(n) => R::Issue,
        ["issues", n, "dependencies", ..] if num(n) => R::Issue,
        ["pulls", "comments"] | ["pulls", "comments", _] => R::ReviewComment,
        ["pulls", _, "comments"] | ["pulls", _, "comments", _, "replies"] => R::ReviewComment,
        ["pulls", _, "reviews", _, "comments"] => R::ReviewComment,
        ["pulls", _, "reviews"] | ["pulls", _, "reviews", _] | ["pulls", _, "reviews", _, "dismissals" | "events"] => {
            R::Review
        }
        ["pulls", _, "commits"] | ["commits"] | ["commits", _] | ["merges"] => R::Commit,
        ["pulls"] | ["pulls", _] | ["pulls", _, "requested_reviewers"] | ["commits", _, "pulls"] => R::PullRequest,
        ["releases"] | ["releases", "latest"] | ["releases", "tags", _] => R::Release,
        ["releases", n] if num(n) => R::Release,
        ["check-runs"] | ["check-runs", _] | ["check-suites", _, "check-runs"] | ["commits", _, "check-runs"] => {
            R::CheckRun
        }
        _ => return None,
    })
}

/// What the edge does to a reply it rendered for the REST path `rest_path` (under `/api/v3`): nothing, unless
/// `accept` opts in and the path is a route of a trust resource. Then the reply varies on `Accept`, and a 2xx JSON
/// body gets one `graftyard_trust` per object.
pub fn layer(accept: Option<&str>, rest_path: &str, r: &Repo<'_>, reply: Reply) -> Reply {
    if !opted_in(accept) {
        return reply;
    }
    let Some(kind) = resource_of(rest_path) else { return reply };
    let reply = reply.with("vary", "Accept");
    if !(200..300).contains(&reply.status) {
        return reply;
    }
    annotate(r, kind, reply)
}

/// `reply` with `graftyard_trust` on each object of `kind` in its JSON body: the body itself, each element of a list,
/// or each run of a `check_runs` list.
pub fn annotate(r: &Repo<'_>, kind: Resource, mut reply: Reply) -> Reply {
    let Body::Json(v) = &mut reply.body else { return reply };
    let one = |m: &mut Map<String, Value>| {
        if let Some(t) = view_of(r, kind, m) {
            m.insert("graftyard_trust".into(), serde_json::to_value(t).expect("a trust view serializes"));
        }
    };
    match v {
        Value::Array(xs) => xs.iter_mut().filter_map(Value::as_object_mut).for_each(one),
        Value::Object(m) if kind == Resource::CheckRun && m.contains_key("check_runs") => {
            if let Some(Value::Array(xs)) = m.get_mut("check_runs") {
                xs.iter_mut().filter_map(Value::as_object_mut).for_each(one);
            }
        }
        Value::Object(m) => one(m),
        _ => {}
    }
    reply
}

/// The view of one rendered object: of the raw text it carries, as every route renders it, and of its author. `None`
/// when the object carries no text field, so a view is never of text the client was not sent.
pub fn view_of(r: &Repo<'_>, kind: Resource, o: &Map<String, Value>) -> Option<TrustView> {
    let text = |v: Option<&Value>| -> Option<String> {
        match v? {
            Value::Null => Some(String::new()),
            Value::String(s) => Some(s.clone()),
            _ => None,
        }
    };
    let account = |key: &str| -> (Option<AccountId>, bool) {
        let u = o.get(key);
        let id = u.and_then(|u| u.get("id")).and_then(Value::as_i64).map(|i| AccountId(DbId(i)));
        (id, u.and_then(|u| u.get("type")).and_then(Value::as_str) == Some("Bot"))
    };
    use Resource as R;
    let (body, (who, bot)) = match kind {
        R::Issue | R::PullRequest | R::IssueComment | R::Review | R::ReviewComment => {
            (text(o.get("body"))?, account("user"))
        }
        R::Release => (text(o.get("body"))?, account("author")),
        // A commit's author is a claim in the git object, not an authenticated account [inference], so no account's
        // permission vouches for its message.
        R::Commit => (text(o.get("commit").and_then(|c| c.get("message")))?, (None, false)),
        // A check run is written by its App.
        R::CheckRun => {
            let out = o.get("output")?;
            let parts: Vec<&str> = ["title", "summary", "text"].iter().filter_map(|k| out.get(*k)?.as_str()).collect();
            (parts.join("\n\n"), (None, true))
        }
    };
    Some(TrustView::of(r, &body, who, bot))
}

impl TrustView {
    /// The view of `text`, written by `who` (`None` for no account) on this repository.
    pub fn of(r: &Repo<'_>, text: &str, who: Option<AccountId>, is_bot: bool) -> TrustView {
        let (visible, hidden) = visible(text);
        TrustView { visible: visible.into(), hidden, author: author_trust(r, who, is_bot) }
    }
}

// ---- from fold rows, for MCP and the controller's prompt ----

pub fn issue(r: &Repo<'_>, it: &Item) -> TrustView {
    TrustView::of(r, &it.row.body, it.row.author, false)
}

/// An issue comment or a review comment.
pub fn comment(r: &Repo<'_>, c: &CommentRow) -> TrustView {
    TrustView::of(r, &c.body, c.author, false)
}

pub fn review(r: &Repo<'_>, v: &ReviewRow) -> TrustView {
    TrustView::of(r, &v.body, v.reviewer, false)
}

pub fn release(r: &Repo<'_>, rel: &ReleaseRow) -> TrustView {
    TrustView::of(r, &rel.rec.body, rel.rec.author, false)
}

/// A check run's output, written by its App.
pub fn check_run(r: &Repo<'_>, c: &CheckRunUpdate) -> TrustView {
    let text = c.output.as_ref().map(|o| {
        let parts: Vec<&str> = [Some(&*o.title), Some(&*o.summary), o.text.as_deref()].into_iter().flatten().collect();
        parts.join("\n\n")
    });
    TrustView::of(r, text.as_deref().unwrap_or(""), None, true)
}

/// `AuthorTrust` of `who` on this repository: it can push if it is the owner or holds `write` or more.
pub fn author_trust(r: &Repo<'_>, who: Option<AccountId>, is_bot: bool) -> AuthorTrust {
    let can_push =
        !is_bot && who.is_some_and(|a| a == r.at.owner_id || r.dir.permission(a) >= Some(RepoPermission::Write));
    let is_agent = false;
    AuthorTrust {
        association: crate::wire::association(r, who),
        can_push,
        is_bot,
        is_agent,
        first_contribution: false,
        trusted: can_push && !is_bot && !is_agent,
    }
}

// ---- visible ----

pub fn is_hidden(c: char) -> bool {
    HIDDEN_CHARS.iter().any(|r| r.contains(&c))
}

fn is_hidden_cp(cp: u32) -> bool {
    char::from_u32(cp).is_some_and(is_hidden)
}

/// Named references (HTML5) that decode into `HIDDEN_CHARS`. Every other named reference stays as text.
const HIDDEN_NAMED: &[(&str, u32)] = &[
    ("shy", 0xad),
    ("zwj", 0x200d),
    ("zwnj", 0x200c),
    ("lrm", 0x200e),
    ("rlm", 0x200f),
    ("ZeroWidthSpace", 0x200b),
    ("NegativeVeryThinSpace", 0x200b),
    ("NegativeThinSpace", 0x200b),
    ("NegativeMediumSpace", 0x200b),
    ("NegativeThickSpace", 0x200b),
    ("NoBreak", 0x2060),
    ("ApplyFunction", 0x2061),
    ("af", 0x2061),
    ("InvisibleTimes", 0x2062),
    ("it", 0x2062),
    ("InvisibleComma", 0x2063),
    ("ic", 0x2063),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ctx {
    Para,
    Html,
    Fence,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Summary {
    None,
    In,
    Done,
}

struct Frame {
    collapsed: bool,
    summary: Summary,
    content: bool,
}

fn byte(s: &str, i: usize) -> Option<u8> {
    s.as_bytes().get(i).copied()
}

fn is_sp(b: Option<u8>) -> bool {
    matches!(b, Some(b' ' | b'\t'))
}

fn is_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

/// The width of the character at byte `i`, a character boundary.
fn width(s: &str, i: usize) -> usize {
    s.get(i..).and_then(|t| t.chars().next()).map_or(1, char::len_utf8)
}

/// `[ \t\r]*` to the end: blank, with a CRLF line's `\r` allowed.
fn blank(s: &str) -> bool {
    s.bytes().all(|b| matches!(b, b' ' | b'\t' | b'\r'))
}

/// Up to three spaces or tabs.
fn indent(s: &str) -> usize {
    s.bytes().take(3).take_while(|b| matches!(b, b' ' | b'\t')).count()
}

/// The text after any blockquote and list markers that can precede a block start.
fn strip_containers(mut s: &str) -> &str {
    loop {
        let i = indent(s);
        let b = s.as_bytes();
        let next = match b.get(i) {
            Some(b'>') => i + 1 + usize::from(is_sp(b.get(i + 1).copied())),
            Some(b'-' | b'*' | b'+') if is_sp(b.get(i + 1).copied()) => {
                i + 1 + b[i + 1..].iter().take_while(|c| matches!(c, b' ' | b'\t')).count()
            }
            Some(d) if d.is_ascii_digit() => {
                let n = b[i..].iter().take(9).take_while(|c| c.is_ascii_digit()).count();
                let j = i + n;
                if matches!(b.get(j), Some(b'.' | b')')) && is_sp(b.get(j + 1).copied()) {
                    j + 1 + b[j + 1..].iter().take_while(|c| matches!(c, b' ' | b'\t')).count()
                } else {
                    return s;
                }
            }
            _ => return s,
        };
        s = &s[next..];
    }
}

/// `<` then a letter, `/` and a letter, `!`, or `?`, after up to three spaces.
fn html_start(s: &str) -> bool {
    let i = indent(s);
    let b = s.as_bytes();
    b.get(i) == Some(&b'<')
        && match b.get(i + 1) {
            Some(c) if c.is_ascii_alphabetic() || *c == b'!' || *c == b'?' => true,
            Some(b'/') => b.get(i + 2).is_some_and(u8::is_ascii_alphabetic),
            _ => false,
        }
}

/// A fence opener: its character, its run length, and the length of the match.
fn fence_at(s: &str) -> Option<(u8, usize, usize)> {
    let i = indent(s);
    let c = byte(s, i).filter(|c| *c == b'`' || *c == b'~')?;
    let n = s.as_bytes()[i..].iter().take_while(|x| **x == c).count();
    (n >= 3).then_some((c, n, i + n))
}

/// Each byte's context: raw HTML, fenced code, or markdown paragraph text. Conservative toward raw HTML.
fn classify(src: &str) -> Vec<Ctx> {
    let n = src.len();
    let mut at = vec![Ctx::Para; n + 1];
    let (mut in_html, mut fence) = (false, None::<(u8, usize)>);
    let mut pos = 0;
    loop {
        let nl = src[pos..].find('\n').map(|k| pos + k);
        let end = nl.unwrap_or(n);
        let line = &src[pos..end];
        let text = line.strip_suffix('\r').unwrap_or(line);
        let stripped = strip_containers(text);
        let mut ctx = Ctx::Para;
        if let Some((c, len)) = fence {
            ctx = Ctx::Fence;
            if let Some((c2, l2, m)) = fence_at(stripped)
                && c2 == c
                && l2 >= len
                && blank(&stripped[m..])
            {
                fence = None;
            }
        } else if in_html {
            if blank(stripped) {
                in_html = false;
            } else {
                ctx = Ctx::Html;
            }
        } else if html_start(stripped) || html_start(text) {
            in_html = true;
            ctx = Ctx::Html;
        } else if let Some((c, l, m)) = fence_at(stripped) {
            // A backtick fence's info string may not contain a backtick.
            if !(c == b'`' && stripped[m..].contains('`')) {
                fence = Some((c, l));
                ctx = Ctx::Fence;
            }
        }
        at[pos..=end].fill(ctx);
        match nl {
            Some(k) => pos = k + 1,
            None => return at,
        }
    }
}

fn escaped(s: &str, i: usize) -> bool {
    s.as_bytes()[..i].iter().rev().take_while(|b| **b == b'\\').count() % 2 == 1
}

/// The `\n` that starts the next blank line at or after `from`, or the end.
fn para_end(s: &str, from: usize) -> usize {
    let b = s.as_bytes();
    let mut k = from;
    while let Some(off) = s[k..].find('\n') {
        let nl = k + off;
        let mut j = nl + 1;
        while matches!(b.get(j), Some(b' ' | b'\t')) {
            j += 1;
        }
        if b.get(j) == Some(&b'\r') {
            j += 1;
        }
        if b.get(j) == Some(&b'\n') {
            return nl;
        }
        k = nl + 1;
    }
    s.len()
}

fn line_end(s: &str, i: usize) -> usize {
    s[i..].find('\n').map_or(s.len(), |k| i + k)
}

/// A footnote definition runs through its first line and any indented continuation lines.
fn def_end(s: &str, i: usize) -> usize {
    let mut e = line_end(s, i);
    while e < s.len() {
        let n = line_end(s, e + 1);
        let next = &s[e + 1..n];
        let lead = if next.starts_with('\t') { 1 } else { next.bytes().take_while(|b| *b == b' ').count() };
        let indented = next.starts_with('\t') || lead >= 2;
        if indented && next[lead..].chars().next().is_some_and(|c| !c.is_whitespace()) {
            e = n;
        } else {
            break;
        }
    }
    e
}

/// `[^label]:` at a line start, after up to three spaces: the label and the match length.
fn footnote_def(s: &str) -> Option<(&str, usize)> {
    let i = indent(s);
    let rest = s.get(i..)?.strip_prefix("[^")?;
    let len: usize = rest.chars().take_while(|c| *c != ']' && !c.is_whitespace()).map(char::len_utf8).sum();
    let label = &rest[..len];
    let n = label.chars().count();
    ((1..=200).contains(&n) && rest[len..].starts_with("]:")).then_some((label, i + 2 + len + 2))
}

/// `[label]:` at a line start, after up to three spaces: the match length.
fn ref_def(s: &str) -> Option<usize> {
    let i = indent(s);
    let rest = s.get(i..)?.strip_prefix('[')?;
    let (mut j, mut n) = (0, 0);
    let b = rest.as_bytes();
    while let Some(c) = b.get(j) {
        match c {
            b'[' => return None,
            b']' => break,
            b'\\' if b.get(j + 1).is_some_and(|x| *x != b'\n') => j += 1 + width(rest, j + 1),
            b'\\' => return None,
            _ => j += width(rest, j),
        }
        n += 1;
    }
    ((1..=999).contains(&n) && rest[j..].starts_with("]:")).then_some(i + 1 + j + 2)
}

/// Every footnote label referred to outside a definition, lowercased.
fn footnote_refs(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut k = 0;
    while let Some(off) = s[k..].find("[^") {
        let at = k + off + 2;
        let len: usize = s[at..].chars().take_while(|c| *c != ']' && !c.is_whitespace()).map(char::len_utf8).sum();
        let n = s[at..at + len].chars().count();
        if (1..=200).contains(&n) && s[at + len..].starts_with(']') && !s[at + len + 1..].starts_with(':') {
            out.push(s[at..at + len].to_lowercase());
            k = at + len + 1;
        } else {
            k = at - 1;
        }
    }
    out
}

fn skip_ws(s: &str, mut i: usize) -> usize {
    let mut lines = 0;
    while let Some(c) = byte(s, i).filter(|c| matches!(c, b' ' | b'\t' | b'\r' | b'\n')) {
        if c == b'\n' {
            lines += 1;
            if lines > 1 {
                break;
            }
        }
        i += 1;
    }
    i
}

/// A link title in `"…"`, `'…'`, or `(…)` at `i`: its text and the end.
fn title_at(s: &str, i: usize) -> Option<(&str, usize)> {
    let open = byte(s, i)?;
    let close = if open == b'(' { b')' } else { open };
    let mut j = i + 1;
    while let Some(c) = byte(s, j) {
        if c == b'\\' {
            j += 1 + width(s, j + 1);
            continue;
        }
        if c == close {
            return Some((&s[i + 1..j], j + 1));
        }
        if open == b'(' && c == b'(' {
            return None;
        }
        if c == b'\n' && para_end(s, j) == j {
            return None;
        }
        j += width(s, j);
    }
    None
}

struct Link<'s> {
    dest: &'s str,
    title: bool,
    end: usize,
}

/// `dest`, an optional title, and `)` after `](`, as CommonMark's inline link grammar reads them.
fn inline_link(s: &str, i: usize) -> Option<Link<'_>> {
    let mut j = skip_ws(s, i);
    let dest;
    if byte(s, j) == Some(b'<') {
        let e = j + 1 + s[j + 1..].find('>')?;
        if s[j + 1..e].contains('\n') {
            return None;
        }
        dest = &s[j..=e];
        j = e + 1;
    } else {
        let (st, mut depth) = (j, 0usize);
        while let Some(c) = byte(s, j) {
            if c == b'\\' && j + 1 < s.len() {
                j += 1 + width(s, j + 1);
                continue;
            }
            if is_ws(c) {
                break;
            }
            if c == b'(' {
                depth += 1;
            }
            if c == b')' {
                if depth == 0 {
                    break;
                }
                depth -= 1;
            }
            j += width(s, j);
        }
        dest = &s[st..j];
    }
    let ws = skip_ws(s, j);
    let mut title = false;
    if ws > j && matches!(byte(s, ws), Some(b'"' | b'\'' | b'(')) {
        let (_, end) = title_at(s, ws)?;
        title = true;
        j = end;
    } else {
        j = ws;
    }
    j = skip_ws(s, j);
    (byte(s, j) == Some(b')')).then_some(Link { dest, title, end: j + 1 })
}

/// A reference definition's destination and optional title after `[label]:`, through the end of its line: whether it
/// has a title, and the end.
fn ref_def_body(s: &str, i: usize) -> Option<(bool, usize)> {
    let mut j = skip_ws(s, i);
    if j >= s.len() {
        return None;
    }
    if byte(s, j) == Some(b'<') {
        j += s[j..].find('>')? + 1;
    } else {
        let st = j;
        while byte(s, j).is_some_and(|c| !is_ws(c)) {
            j += width(s, j);
        }
        if j == st {
            return None;
        }
    }
    let after_dest = j;
    let ws = skip_ws(s, j);
    if ws > j
        && matches!(byte(s, ws), Some(b'"' | b'\'' | b'('))
        && let Some((_, end)) = title_at(s, ws)
    {
        let e = line_end(s, end);
        if blank(&s[end..e]) {
            return Some((true, e));
        }
    }
    let e = line_end(s, after_dest);
    blank(&s[after_dest..e]).then_some((false, e))
}

/// The end of the first `close` at or after `from`; in raw-HTML context the end of the text when there is none, and in
/// a paragraph `None` (literal text) unless it closes before `limit`.
fn close_of(s: &str, from: usize, close: &str, limit: usize, ctx: Ctx) -> Option<usize> {
    match s[from..].find(close).map(|k| from + k) {
        Some(e) if ctx == Ctx::Html || e < limit => Some(e + close.len()),
        _ if ctx == Ctx::Html => Some(s.len()),
        _ => None,
    }
}

/// The closing backtick run of exactly `n` before `limit`.
fn code_close(s: &str, from: usize, n: usize, limit: usize) -> Option<usize> {
    let mut j = from;
    while j < limit {
        let k = j + s[j..].find('`')?;
        if k >= limit {
            return None;
        }
        let r = s.as_bytes()[k..].iter().take_while(|b| **b == b'`').count();
        if r == n {
            return Some(k);
        }
        j = k + r;
    }
    None
}

struct Tag<'s> {
    name: String,
    close: bool,
    attrs: Vec<(String, &'s str)>,
    end: usize,
}

impl Tag<'_> {
    fn has(&self, name: &str) -> bool {
        self.attrs.iter().any(|(n, _)| n == name)
    }
}

/// An HTML open or close tag at `i`, by CommonMark's raw HTML tag grammar with quotes honored.
fn tag_at(s: &str, i: usize) -> Option<Tag<'_>> {
    let b = s.as_bytes();
    let mut j = i + 1;
    let close = b.get(j) == Some(&b'/');
    j += usize::from(close);
    if !b.get(j)?.is_ascii_alphabetic() {
        return None;
    }
    let st = j;
    while b.get(j).is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'-') {
        j += 1;
    }
    let name = s[st..j].to_owned();
    let mut attrs = Vec::new();
    if close {
        while b.get(j).is_some_and(|c| is_ws(*c)) {
            j += 1;
        }
        return (b.get(j) == Some(&b'>')).then_some(Tag { name, close, attrs, end: j + 1 });
    }
    loop {
        let ws = b[j..].iter().take_while(|c| is_ws(**c)).count();
        j += ws;
        match b.get(j) {
            Some(b'>') => return Some(Tag { name, close, attrs, end: j + 1 }),
            Some(b'/') if b.get(j + 1) == Some(&b'>') => return Some(Tag { name, close, attrs, end: j + 2 }),
            _ if ws == 0 => return None,
            Some(c) if c.is_ascii_alphabetic() || *c == b'_' || *c == b':' => {}
            _ => return None,
        }
        let an = j;
        while b.get(j).is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b':' | b'-')) {
            j += 1;
        }
        let attr = s[an..j].to_ascii_lowercase();
        let mut value = "";
        let mut k = j;
        while b.get(k).is_some_and(|c| is_ws(*c)) {
            k += 1;
        }
        if b.get(k) == Some(&b'=') {
            k += 1;
            while b.get(k).is_some_and(|c| is_ws(*c)) {
                k += 1;
            }
            match b.get(k) {
                Some(q @ (b'"' | b'\'')) => {
                    let e = k + 1 + s[k + 1..].find(*q as char)?;
                    value = &s[k + 1..e];
                    j = e + 1;
                }
                _ => {
                    let st = k;
                    while b.get(k).is_some_and(|c| !is_ws(*c) && !matches!(c, b'"' | b'\'' | b'=' | b'<' | b'>' | b'`'))
                    {
                        k += 1;
                    }
                    if k == st {
                        return None;
                    }
                    value = &s[st..k];
                    j = k;
                }
            }
        }
        attrs.push((attr, value));
    }
}

/// A character reference at `i`: its length and the code point it names, `None` for a named one that is not hidden.
fn char_ref(s: &str, i: usize) -> Option<(usize, Option<u32>)> {
    let b = s.as_bytes();
    let digits = |from: usize, max: usize, hex: bool| {
        let n =
            b[from.min(b.len())..].iter().take_while(|c| if hex { c.is_ascii_hexdigit() } else { c.is_ascii_digit() });
        let n = n.count();
        ((1..=max).contains(&n) && b.get(from + n) == Some(&b';')).then_some(n)
    };
    if b.get(i + 1) == Some(&b'#') {
        if matches!(b.get(i + 2), Some(b'x' | b'X')) {
            let n = digits(i + 3, 6, true)?;
            return Some((n + 4, u32::from_str_radix(&s[i + 3..i + 3 + n], 16).ok()));
        }
        let n = digits(i + 2, 7, false)?;
        return Some((n + 3, s[i + 2..i + 2 + n].parse().ok()));
    }
    if !b.get(i + 1)?.is_ascii_alphabetic() {
        return None;
    }
    let n = b[i + 2..].iter().take_while(|c| c.is_ascii_alphanumeric()).count();
    if !(1..=31).contains(&n) || b.get(i + 2 + n) != Some(&b';') {
        return None;
    }
    let name = &s[i + 1..i + 2 + n];
    Some((n + 3, HIDDEN_NAMED.iter().find(|(k, _)| *k == name).map(|(_, cp)| *cp)))
}

fn details_at(s: &str, i: usize) -> bool {
    s.get(i + 1..i + 8).is_some_and(|t| t.eq_ignore_ascii_case("details"))
        && byte(s, i + 8).is_some_and(|c| is_ws(c) || c == b'>' || c == b'/')
        && byte(s, i) == Some(b'<')
}

struct Scan<'s> {
    src: &'s str,
    hidden: Hidden,
    out: String,
    stack: Vec<Frame>,
}

impl Scan<'_> {
    fn shown(&self) -> bool {
        self.stack.iter().all(|f| !f.collapsed || f.summary == Summary::In)
    }

    fn emit(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        if !s.chars().all(char::is_whitespace) {
            for f in &mut self.stack {
                if f.summary != Summary::In {
                    f.content = true;
                }
            }
        }
        if self.shown() {
            self.out.push_str(s);
        }
    }

    /// `s` without its hidden characters, raw or as references, each counted.
    fn strip(&mut self, s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let mut i = 0;
        while i < s.len() {
            if byte(s, i) == Some(b'&')
                && let Some((n, Some(cp))) = char_ref(s, i)
                && is_hidden_cp(cp)
            {
                self.hidden.invisible_chars += 1;
                i += n;
                continue;
            }
            let w = width(s, i);
            let c = &s[i..i + w];
            if c.chars().next().is_some_and(is_hidden) {
                self.hidden.invisible_chars += 1;
            } else {
                out.push_str(c);
            }
            i += w;
        }
        out
    }

    /// A `<details` inside code: counted as an open, so the region it may start is never shown by a later close.
    fn open_details(&mut self, at: usize) {
        let open = tag_at(self.src, at).is_some_and(|t| t.has("open"));
        if !open {
            self.hidden.collapsed += 1;
        }
        self.stack.push(Frame { collapsed: !open, summary: Summary::None, content: false });
    }

    /// One raw hidden character at `i` is counted, any other is emitted; returns its width.
    fn char_at(&mut self, i: usize) -> usize {
        let w = width(self.src, i);
        let c = &self.src[i..i + w];
        if c.chars().next().is_some_and(is_hidden) {
            self.hidden.invisible_chars += 1;
        } else {
            self.emit(c);
        }
        w
    }

    fn tag(&mut self, t: &Tag<'_>) {
        let name = t.name.to_ascii_lowercase();
        if t.attrs.iter().any(|(n, v)| n == "title" && !v.trim().is_empty()) {
            self.hidden.link_title_text += 1;
        }
        let dropped = t
            .attrs
            .iter()
            .any(|(n, v)| !matches!(n.as_str(), "href" | "src" | "title" | "open") && !v.trim().is_empty());
        if dropped {
            self.hidden.link_title_text += 1;
        }
        let text = if t.close {
            format!("</{name}>")
        } else {
            let mut x = format!("<{name}");
            for (n, v) in t.attrs.iter().filter(|(n, _)| n == "href" || n == "src") {
                let v = self.strip(v);
                x.push_str(&format!(" {n}=\"{v}\""));
            }
            if name == "details" && t.has("open") {
                x.push_str(" open");
            }
            x.push('>');
            x
        };
        let top = self.stack.last().map(|f| (f.summary, f.content));
        match (name.as_str(), t.close) {
            ("details", false) => {
                let collapsed = !t.has("open");
                self.emit(&text);
                if collapsed {
                    self.hidden.collapsed += 1;
                }
                self.stack.push(Frame { collapsed, summary: Summary::None, content: false });
            }
            ("details", true) => {
                self.emit(&text);
                self.stack.pop();
            }
            ("summary", false) if top == Some((Summary::None, false)) => {
                self.stack.last_mut().expect("a frame").summary = Summary::In;
                self.emit(&text);
            }
            ("summary", true) if top.is_some_and(|(s, _)| s == Summary::In) => {
                self.emit(&text);
                self.stack.last_mut().expect("a frame").summary = Summary::Done;
            }
            _ => self.emit(&text),
        }
    }
}

/// `visible` and `hidden` for one text.
pub fn visible(src: &str) -> (String, Hidden) {
    let at = classify(src);
    let refs = footnote_refs(src);
    let mut sc = Scan { src, hidden: Hidden::default(), out: String::with_capacity(src.len()), stack: Vec::new() };
    let n = src.len();
    let (mut i, mut line_start) = (0, true);
    while i < n {
        let ctx = at[i];
        let c = src.as_bytes()[i];
        // ---- Line starts: definitions (markdown context only) ----
        if line_start && ctx == Ctx::Para {
            line_start = false;
            let rest = &src[i..];
            let foot = footnote_def(rest);
            if let Some((label, _)) = foot
                && !refs.contains(&label.to_lowercase())
            {
                sc.hidden.html_comments += 1;
                i = def_end(src, i);
                continue;
            }
            if foot.is_none()
                && let Some(m) = ref_def(rest)
                && let Some((title, end)) = ref_def_body(src, i + m)
            {
                if title {
                    sc.hidden.link_title_text += 1;
                } else {
                    sc.hidden.html_comments += 1;
                }
                i = end;
                continue;
            }
        }
        if c == b'\n' {
            sc.emit("\n");
            i += 1;
            line_start = true;
            continue;
        }
        line_start = false;
        // ---- Fenced code: literal, except raw hidden characters ----
        if ctx == Ctx::Fence {
            let w = sc.char_at(i);
            // `<details` opens still count in code, so a fake close elsewhere cannot end a real block early.
            if details_at(src, i) {
                sc.open_details(i);
            }
            i += w;
            continue;
        }
        // ---- Code spans (paragraph context only): literal ----
        if c == b'`' && ctx == Ctx::Para && !escaped(src, i) {
            let run = src.as_bytes()[i..].iter().take_while(|b| **b == b'`').count();
            match code_close(src, i + run, run, para_end(src, i)) {
                Some(close) => {
                    let mut j = i;
                    while j < close + run {
                        if details_at(src, j) {
                            sc.open_details(j);
                            sc.emit("<");
                            j += 1;
                        } else {
                            j += sc.char_at(j);
                        }
                    }
                    i = close + run;
                }
                None => {
                    sc.emit(&src[i..i + run]);
                    i += run;
                }
            }
            continue;
        }
        // ---- Raw HTML: comments and their relatives ----
        if c == b'<' {
            let limit = if ctx == Ctx::Html { n } else { para_end(src, i) };
            if src[i..].starts_with("<!--") {
                let e = if src[i + 4..].starts_with('>') {
                    Some(i + 5)
                } else if src[i + 4..].starts_with("->") {
                    Some(i + 6)
                } else {
                    let a = src[i + 4..].find("-->").map(|k| i + 4 + k + 3);
                    let b = src[i + 4..].find("--!>").map(|k| i + 4 + k + 4);
                    match a.into_iter().chain(b).min() {
                        None if ctx == Ctx::Html => Some(n),
                        None => None,
                        Some(e) if ctx != Ctx::Html && e > limit => None,
                        Some(e) => Some(e),
                    }
                };
                match e {
                    Some(e) => {
                        sc.hidden.html_comments += 1;
                        i = e;
                    }
                    None => {
                        sc.emit("<");
                        i += 1;
                    }
                }
                continue;
            }
            let other = if src[i..].starts_with("<?") {
                Some(close_of(src, i + 2, "?>", limit, ctx))
            } else if src[i..].starts_with("<![CDATA[") {
                Some(close_of(src, i + 9, "]]>", limit, ctx))
            } else if src[i..].starts_with("<!") && byte(src, i + 2).is_some_and(|b| b.is_ascii_alphabetic()) {
                Some(close_of(src, i + 2, ">", limit, ctx))
            } else {
                None
            };
            match other {
                Some(Some(e)) => {
                    sc.hidden.html_comments += 1;
                    i = e;
                }
                Some(None) => {
                    sc.emit("<");
                    i += 1;
                }
                None => match tag_at(src, i) {
                    Some(t) => {
                        sc.tag(&t);
                        i = t.end;
                    }
                    None => {
                        sc.emit("<");
                        i += 1;
                    }
                },
            }
            continue;
        }
        // ---- Character references ----
        if c == b'&'
            && let Some((len, cp)) = char_ref(src, i)
        {
            if cp.is_some_and(is_hidden_cp) {
                sc.hidden.invisible_chars += 1;
            } else {
                sc.emit(&src[i..i + len]);
            }
            i += len;
            continue;
        }
        // ---- Markdown inline link and image titles (paragraph context) ----
        if c == b']'
            && ctx == Ctx::Para
            && byte(src, i + 1) == Some(b'(')
            && !escaped(src, i)
            && let Some(l) = inline_link(src, i + 2)
        {
            sc.emit("](");
            let dest = sc.strip(l.dest);
            sc.emit(&dest);
            sc.emit(")");
            if l.title {
                sc.hidden.link_title_text += 1;
            }
            i = l.end;
            continue;
        }
        // ---- Plain characters ----
        i += sc.char_at(i);
    }
    (sc.out, sc.hidden)
}
