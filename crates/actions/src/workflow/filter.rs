//! Whether a committed event triggers a workflow: `on:` filters with GitHub's pattern syntax ("Filter pattern cheat
//! sheet"). The pattern semantics are a port of nektos/act's `pkg/workflowpattern` (MIT): `*` matches within a path
//! segment, `**` across them, `?` and `+` repeat the character before them, `[...]` is a class, `\` escapes, and a
//! leading `!` negates, the last matching pattern deciding.

use serde_json::{Map, Value};

#[derive(Clone, Debug, PartialEq)]
enum Atom {
    Lit(char),
    Class(Vec<(char, char)>),
    /// `*`: any run without `/`.
    Segment,
    /// `**`: any run.
    Any,
    /// `**/`: empty, or any run ending in `/`.
    Dirs,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Rep {
    One,
    Opt,
    Plus,
}

/// A compiled pattern.
#[derive(Clone, Debug)]
pub struct Pattern {
    atoms: Vec<(Atom, Rep)>,
    pub negative: bool,
}

impl Pattern {
    pub fn compile(raw: &str) -> Result<Pattern, String> {
        let (negative, p) = match raw.strip_prefix('!') {
            Some(p) => (true, p),
            None => (false, raw),
        };
        let cs: Vec<char> = p.chars().collect();
        let mut atoms: Vec<(Atom, Rep)> = Vec::new();
        let mut i = 0;
        let bad = |why: &str| format!("invalid Pattern '{p}': {why}");
        while i < cs.len() {
            match cs[i] {
                '*' if cs.get(i + 1) == Some(&'*') && cs.get(i + 2) == Some(&'/') => {
                    atoms.push((Atom::Dirs, Rep::One));
                    i += 3;
                }
                '*' if cs.get(i + 1) == Some(&'*') => {
                    atoms.push((Atom::Any, Rep::One));
                    i += 2;
                }
                '*' => {
                    atoms.push((Atom::Segment, Rep::One));
                    i += 1;
                }
                c @ ('+' | '?') if i > 0 => {
                    if let Some(last) = atoms.last_mut()
                        && last.1 == Rep::One
                        && matches!(last.0, Atom::Lit(_) | Atom::Class(_))
                    {
                        last.1 = if c == '+' { Rep::Plus } else { Rep::Opt };
                    }
                    i += 1;
                }
                '[' => {
                    i += 1;
                    let mut ranges = Vec::new();
                    if cs.get(i) == Some(&']') {
                        return Err(bad("Unexpected empty brackets '[]'"));
                    }
                    while i < cs.len() && cs[i] != ']' {
                        if cs[i] == '-' && !ranges.is_empty() && i + 1 < cs.len() && cs[i + 1] != ']' {
                            let (lo, _) = ranges.pop().unwrap_or((cs[i - 1], cs[i - 1]));
                            ranges.push((lo, cs[i + 1]));
                            i += 2;
                        } else {
                            if !cs[i].is_ascii_alphanumeric() && !('A'..='z').contains(&cs[i]) {
                                return Err(bad("Ranges can only include a-z, A-Z and 0-9"));
                            }
                            ranges.push((cs[i], cs[i]));
                            i += 1;
                        }
                    }
                    if i >= cs.len() {
                        return Err(bad("Missing closing bracket ']' after '['"));
                    }
                    i += 1;
                    atoms.push((Atom::Class(ranges), Rep::One));
                }
                '\\' => {
                    let Some(&c) = cs.get(i + 1) else { return Err(bad("Missing symbol after \\")) };
                    atoms.push((Atom::Lit(c), Rep::One));
                    i += 2;
                }
                c => {
                    atoms.push((Atom::Lit(c), Rep::One));
                    i += 1;
                }
            }
        }
        Ok(Pattern { atoms, negative })
    }

    pub fn matches(&self, s: &str) -> bool {
        let cs: Vec<char> = s.chars().collect();
        go(&self.atoms, &cs)
    }
}

fn one(a: &Atom, c: char) -> bool {
    match a {
        Atom::Lit(l) => *l == c,
        Atom::Class(r) => r.iter().any(|(lo, hi)| (*lo..=*hi).contains(&c)),
        _ => false,
    }
}

fn go(atoms: &[(Atom, Rep)], s: &[char]) -> bool {
    let Some(((a, rep), rest)) = atoms.split_first() else { return s.is_empty() };
    match a {
        Atom::Segment => {
            let n = s.iter().take_while(|c| **c != '/').count();
            (0..=n).rev().any(|k| go(rest, &s[k..]))
        }
        Atom::Any => (0..=s.len()).rev().any(|k| go(rest, &s[k..])),
        Atom::Dirs => go(rest, s) || (2..=s.len()).rev().any(|k| s[k - 1] == '/' && go(rest, &s[k..])),
        _ => {
            let n = s.iter().take_while(|c| one(a, **c)).count();
            let (lo, hi) = match rep {
                Rep::One => (1, 1),
                Rep::Opt => (0, 1),
                Rep::Plus => (1, n),
            };
            (lo..=hi.min(n)).rev().any(|k| go(rest, &s[k..]))
        }
    }
}

fn compile_all(v: Option<&Value>) -> Option<Vec<Pattern>> {
    let a = v?.as_array()?;
    Some(a.iter().filter_map(|p| p.as_str()).filter_map(|p| Pattern::compile(p).ok()).collect())
}

/// `branches`, `tags`, `paths`: some input must match, the last matching pattern deciding (act's `Skip`).
fn included(ps: &[Pattern], inputs: &[&str]) -> bool {
    inputs.iter().any(|s| {
        let mut m = false;
        for p in ps {
            if p.matches(s) {
                m = !p.negative;
            }
        }
        m
    })
}

/// `*-ignore`: skipped when every input matches an ignore pattern (act's `Filter`).
fn ignored(ps: &[Pattern], inputs: &[&str]) -> bool {
    !inputs.is_empty() && inputs.iter().all(|s| ps.iter().any(|p| p.matches(s) != p.negative))
}

/// One filter pair over `inputs`: the include list if given, else the ignore list, else everything passes.
fn pair(f: &Map<String, Value>, key: &str, inputs: &[&str]) -> bool {
    if let Some(ps) = compile_all(f.get(key)) {
        return included(&ps, inputs);
    }
    if let Some(ps) = compile_all(f.get(&format!("{key}-ignore"))) {
        return !ignored(&ps, inputs);
    }
    true
}

/// The committed event `ActionsTrigger` asks about. `paths` is `None` when the changed paths are not known, and then
/// path filters pass [inference: GitHub runs a workflow when it cannot compute the diff, as for a new branch].
#[derive(Clone, Copy, Debug)]
pub enum Trigger<'a> {
    Push { r#ref: &'a str, paths: Option<&'a [&'a str]> },
    PullRequest { action: &'a str, base: &'a str, paths: Option<&'a [&'a str]> },
    MergeGroup { action: &'a str, base: &'a str },
    Dispatch,
}

impl Trigger<'_> {
    pub fn event_name(&self) -> &'static str {
        match self {
            Trigger::Push { .. } => "push",
            Trigger::PullRequest { .. } => "pull_request",
            Trigger::MergeGroup { .. } => "merge_group",
            Trigger::Dispatch => "workflow_dispatch",
        }
    }
}

fn has(f: &Map<String, Value>, key: &str) -> bool {
    f.contains_key(key) || f.contains_key(&format!("{key}-ignore"))
}

fn activity(f: &Map<String, Value>, action: &str, defaults: &[&str]) -> bool {
    match f.get("types").and_then(Value::as_array) {
        Some(ts) => ts.iter().any(|t| t.as_str() == Some(action)),
        None => defaults.contains(&action),
    }
}

/// Whether a workflow whose parsed `on:` is `events` runs for `t`.
pub fn triggers(events: &Map<String, Value>, t: &Trigger<'_>) -> bool {
    let Some(f) = events.get(t.event_name()).and_then(Value::as_object) else { return false };
    let paths_ok = |paths: Option<&[&str]>| paths.is_none_or(|p| pair(f, "paths", p));
    match *t {
        Trigger::Push { r#ref, paths } => {
            if let Some(b) = r#ref.strip_prefix("refs/heads/") {
                if !has(f, "branches") && has(f, "tags") {
                    return false;
                }
                pair(f, "branches", &[b]) && paths_ok(paths)
            } else if let Some(tag) = r#ref.strip_prefix("refs/tags/") {
                // Path filters do not apply to tag pushes (GitHub, "Running your workflow only when a push of
                // specific tags occurs").
                if !has(f, "tags") && has(f, "branches") {
                    return false;
                }
                pair(f, "tags", &[tag])
            } else {
                false
            }
        }
        Trigger::PullRequest { action, base, paths } => {
            activity(f, action, &["opened", "synchronize", "reopened"])
                && pair(f, "branches", &[base.strip_prefix("refs/heads/").unwrap_or(base)])
                && paths_ok(paths)
        }
        Trigger::MergeGroup { action, base } => {
            activity(f, action, &["checks_requested"])
                && pair(f, "branches", &[base.strip_prefix("refs/heads/").unwrap_or(base)])
        }
        Trigger::Dispatch => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(p: &str, s: &str) -> bool {
        Pattern::compile(p).unwrap().matches(s)
    }

    /// GitHub's cheat-sheet rows.
    #[test]
    fn patterns_follow_the_cheat_sheet() {
        assert!(m("feature/*", "feature/my-branch"));
        assert!(!m("feature/*", "feature/your/branch"));
        assert!(m("feature/**", "feature/your/branch"));
        assert!(m("main", "main") && !m("main", "mainline"));
        assert!(m("v2*", "v2.0") && m("v2*", "v2"));
        assert!(m("v[12].[0-9]+.[0-9]+", "v1.10.1") && !m("v[12].[0-9]+.[0-9]+", "v3.1.1"));
        assert!(m("**.js", "app.js") && m("**.js", "src/js/app.js"));
        assert!(m("*.js", "app.js") && !m("*.js", "src/app.js"));
        assert!(m("docs/**", "docs/a/b.md") && !m("docs/**", "README.md"));
        assert!(m("**/docs/**", "docs/hello.md") && m("**/docs/**", "a/docs/x.md"));
        assert!(m("**/README.md", "README.md") && m("**/README.md", "js/README.md"));
        assert!(m("*.jsx?", "page.js") && m("*.jsx?", "page.jsx") && !m("*.jsx?", "page.jsxx"));
        assert!(m("\\!file", "!file"));
        assert!(!m(".yml", "fyml"));
        assert!(Pattern::compile("[]").is_err());
    }

    fn on(yaml: &str) -> Map<String, Value> {
        let p =
            super::super::parse("t.yml", &format!("{yaml}\njobs:\n  a:\n    runs-on: x\n    steps: [{{run: x}}]\n"));
        assert!(p.ok(), "{:?}", p.errors);
        p.workflow.events.unwrap()
    }

    #[test]
    fn push_branches_tags_and_paths() {
        let e = on(
            "on:\n  push:\n    branches: [main, 'releases/**', '!releases/**-alpha']\n    paths-ignore: ['docs/**']",
        );
        let push = |r, p: &[&str]| triggers(&e, &Trigger::Push { r#ref: r, paths: Some(p) });
        assert!(push("refs/heads/main", &["src/a.rs"]));
        assert!(push("refs/heads/releases/1.0", &["src/a.rs"]));
        assert!(!push("refs/heads/releases/1.0-alpha", &["src/a.rs"]));
        assert!(!push("refs/heads/dev", &["src/a.rs"]));
        assert!(!push("refs/heads/main", &["docs/a.md", "docs/b.md"]));
        assert!(push("refs/heads/main", &["docs/a.md", "src/b.rs"]));
        // Only branches given: a tag push does not run it.
        assert!(!push("refs/tags/v1", &[]));
        let any = on("on: push");
        assert!(triggers(&any, &Trigger::Push { r#ref: "refs/tags/v1", paths: None }));
        let tags = on("on:\n  push:\n    tags: ['v*']\n    paths: [src/**]");
        assert!(triggers(&tags, &Trigger::Push { r#ref: "refs/tags/v2", paths: Some(&["docs/x"]) }));
        assert!(!triggers(&tags, &Trigger::Push { r#ref: "refs/heads/main", paths: Some(&["src/x"]) }));
    }

    #[test]
    fn pull_request_and_merge_group_types() {
        let e = on("on:\n  pull_request:\n    branches: [main]\n  merge_group:");
        let pr = |a, b| triggers(&e, &Trigger::PullRequest { action: a, base: b, paths: None });
        assert!(pr("opened", "main") && pr("synchronize", "refs/heads/main"));
        assert!(!pr("enqueued", "main") && !pr("opened", "dev"));
        assert!(triggers(&e, &Trigger::MergeGroup { action: "checks_requested", base: "refs/heads/main" }));
        assert!(!triggers(&e, &Trigger::Push { r#ref: "refs/heads/main", paths: None }));
        let q = on("on:\n  pull_request:\n    types: [enqueued, dequeued]\n  merge_group:\n    branches: [release]");
        assert!(triggers(&q, &Trigger::PullRequest { action: "dequeued", base: "x", paths: None }));
        assert!(!triggers(&q, &Trigger::PullRequest { action: "opened", base: "x", paths: None }));
        assert!(!triggers(&q, &Trigger::MergeGroup { action: "checks_requested", base: "main" }));
    }
}
