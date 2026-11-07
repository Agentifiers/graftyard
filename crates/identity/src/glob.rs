//! `PathGlob` and `RefPattern` (`design/DESIGN.md`, Shape, \"Agent load and agent safety\", the lease block):
//! gitignore-style, `*` within one path segment and `**` across segments, every glob anchored at the root unless it
//! starts with `**/`, and no negation. A `RefPattern` is the same syntax over full ref names. Ported from the M0-WP10
//! prototype (`tests/m0/src/agent/glob.ts`), which B0-36 ran.

use serde::{Deserialize, Serialize};

/// A ref name pattern, such as an `AgentApp`'s `confine` (`refs/heads/claude/**`).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct RefPattern(Box<str>);

/// A path pattern over a tree, such as an entry of `PROTECTED_PATHS`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct PathGlob(Box<str>);

/// At most this many bytes in a pattern [guess: a ref name is at most 900 bytes (`design/SHARED.md`, `RefName`)].
pub const GLOB_MAX: usize = 900;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum GlobError {
    #[error("a pattern is 1 to 900 bytes with no empty segment")]
    Shape,
    #[error("a ref pattern starts with refs/")]
    NotRefs,
}

fn shape_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= GLOB_MAX && !s.starts_with('/') && !s.contains("//") && !s.contains('\0')
}

impl RefPattern {
    pub fn parse(s: &str) -> Result<RefPattern, GlobError> {
        if !shape_ok(s) {
            return Err(GlobError::Shape);
        }
        if !s.starts_with("refs/") {
            return Err(GlobError::NotRefs);
        }
        Ok(RefPattern(s.into()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn matches(&self, refname: &[u8]) -> bool {
        std::str::from_utf8(refname).is_ok_and(|r| glob_match(&self.0, r))
    }
}

impl PathGlob {
    pub fn parse(s: &str) -> Result<PathGlob, GlobError> {
        if shape_ok(s) { Ok(PathGlob(s.into())) } else { Err(GlobError::Shape) }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn matches(&self, path: &str) -> bool {
        glob_match(&self.0, path)
    }
}

macro_rules! string_forms {
    ($t:ident) => {
        impl From<$t> for String {
            fn from(p: $t) -> String {
                p.0.into()
            }
        }
        impl TryFrom<String> for $t {
            type Error = GlobError;
            fn try_from(s: String) -> Result<$t, GlobError> {
                $t::parse(&s)
            }
        }
    };
}
string_forms!(RefPattern);
string_forms!(PathGlob);

fn segments(s: &str) -> Vec<&str> {
    // A trailing `/` means everything under it.
    let mut v: Vec<&str> = s.trim_end_matches('/').split('/').collect();
    if s.ends_with('/') {
        v.push("**");
    }
    v
}

/// One segment against one pattern segment, where `*` matches any run of characters but `/`.
fn seg_match(pat: &str, s: &str) -> bool {
    let parts: Vec<&str> = pat.split('*').collect();
    if parts.len() == 1 {
        return pat == s;
    }
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if !s.starts_with(first) || s.len() < first.len() + last.len() || !s.ends_with(last) {
        return false;
    }
    let mut rest = &s[first.len()..s.len() - last.len()];
    for mid in &parts[1..parts.len() - 1] {
        match rest.find(mid) {
            Some(i) => rest = &rest[i + mid.len()..],
            None => return false,
        }
    }
    true
}

fn match_segs(g: &[&str], p: &[&str]) -> bool {
    match g.first() {
        None => p.is_empty(),
        Some(&"**") => (0..=p.len()).any(|k| match_segs(&g[1..], &p[k..])),
        Some(seg) => !p.is_empty() && seg_match(seg, p[0]) && match_segs(&g[1..], &p[1..]),
    }
}

/// Whether `path` (a file path or a full ref name) matches `glob`.
pub fn glob_match(glob: &str, path: &str) -> bool {
    match_segs(&segments(glob), &path.split('/').collect::<Vec<_>>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs_anchor_at_the_root_and_star_stays_in_one_segment() {
        assert!(glob_match(".github/workflows/**", ".github/workflows/ci.yml"));
        assert!(glob_match(".github/workflows/**", ".github/workflows/a/b.yml"));
        assert!(!glob_match(".github/workflows/**", "x/.github/workflows/ci.yml"));
        assert!(glob_match("**/AGENTS.md", "AGENTS.md"));
        assert!(glob_match("**/AGENTS.md", "pkg/a/AGENTS.md"));
        assert!(!glob_match("**/AGENTS.md", "pkg/AGENTS.md.bak"));
        assert!(glob_match("CODEOWNERS", "CODEOWNERS"));
        assert!(!glob_match("CODEOWNERS", "docs/CODEOWNERS"));
        assert!(glob_match("src/*.rs", "src/a.rs"));
        assert!(!glob_match("src/*.rs", "src/a/b.rs"));
        assert!(glob_match("out/", "out/x/y"));
        let claude = RefPattern::parse("refs/heads/claude/**").unwrap();
        assert!(claude.matches(b"refs/heads/claude/fix-1"));
        assert!(claude.matches(b"refs/heads/claude/a/b"));
        assert!(!claude.matches(b"refs/heads/main"));
        assert!(!claude.matches(b"refs/heads/claudex/a"));
        assert_eq!(RefPattern::parse("heads/x"), Err(GlobError::NotRefs));
        assert_eq!(RefPattern::parse("refs//x"), Err(GlobError::Shape));
    }
}
