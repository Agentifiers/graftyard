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

/// Whether segments `p` match pattern segments `g`, in O(|g|·|p|) rather than backtracking over every `**`: after
/// the first `i` pattern segments, `ok[j]` is whether they match `p[..j]`.
fn match_segs(g: &[&str], p: &[&str]) -> bool {
    let mut ok = vec![false; p.len() + 1];
    ok[0] = true;
    let mut next = vec![false; p.len() + 1];
    for seg in g {
        for j in 0..=p.len() {
            next[j] = if *seg == "**" {
                ok[j] || (j > 0 && next[j - 1])
            } else {
                j > 0 && ok[j - 1] && seg_match(seg, p[j - 1])
            };
        }
        std::mem::swap(&mut ok, &mut next);
    }
    ok[p.len()]
}

/// Whether `path` (a file path or a full ref name) matches `glob`.
pub fn glob_match(glob: &str, path: &str) -> bool {
    match_segs(&segments(glob), &path.split('/').collect::<Vec<_>>())
}

/// Whether two single segments overlap: some string matches both. A literal against a pattern is exact. Two patterns
/// compare their literal prefixes and suffixes, which is exact for one `*` each and conservative (it may say they
/// overlap when they do not) beyond that, as the M0-WP12 prototype's `segOverlap` does.
fn seg_overlap(a: &str, b: &str) -> bool {
    match (a.contains('*'), b.contains('*')) {
        (false, false) => a == b,
        (false, true) => seg_match(b, a),
        (true, false) => seg_match(a, b),
        (true, true) => {
            let (pa, pb) = (&a[..a.find('*').unwrap_or(0)], &b[..b.find('*').unwrap_or(0)]);
            let (sa, sb) = (&a[a.rfind('*').map_or(0, |i| i + 1)..], &b[b.rfind('*').map_or(0, |i| i + 1)..]);
            (pa.starts_with(pb) || pb.starts_with(pa)) && (sa.ends_with(sb) || sb.ends_with(sa))
        }
    }
}

/// Whether some path matches both segment lists, in O(|a|·|b|) rather than backtracking over every `**`: filling
/// rows from the end, `row[j]` is whether `a[i..]` and `b[j..]` overlap, and `below` holds row `i + 1`.
fn overlap_segs(a: &[&str], b: &[&str]) -> bool {
    let mut below = vec![false; b.len() + 1];
    let mut row = vec![false; b.len() + 1];
    for i in (0..=a.len()).rev() {
        for j in (0..=b.len()).rev() {
            row[j] = match (a.get(i), b.get(j)) {
                (None, None) => true,
                (Some(&"**"), _) => below[j] || (j < b.len() && row[j + 1]),
                (_, Some(&"**")) => row[j + 1] || (i < a.len() && below[j]),
                (Some(x), Some(y)) => seg_overlap(x, y) && below[j + 1],
                _ => false,
            };
        }
        std::mem::swap(&mut below, &mut row);
    }
    below[0]
}

/// Whether some path matches both globs, found by walking both segment lists (`design/DESIGN.md`, Shape, the lease
/// block: "Two globs overlap when some path matches both"). A trailing `/` means everything under it, so a changed
/// directory past `PATHS_MAX` (`truncated`) written as `dir/` overlaps every glob under it.
pub fn glob_overlap(a: &str, b: &str) -> bool {
    overlap_segs(&segments(a), &segments(b))
}

/// A glob's literal prefix: its segments before the first one with a wildcard, joined. The index key of an Exclusive
/// lease's globs (`LeaseEnforce`: "Exclusive globs are indexed by literal prefix").
pub fn literal_prefix(glob: &str) -> String {
    segments(glob).into_iter().take_while(|s| !s.contains('*')).collect::<Vec<_>>().join("/")
}

impl PathGlob {
    /// `glob_overlap` of two globs.
    pub fn overlaps(&self, other: &str) -> bool {
        glob_overlap(&self.0, other)
    }
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

    #[test]
    fn two_globs_overlap_when_some_path_matches_both() {
        let yes = [
            ("pkg/auth/**", "pkg/auth/session.go"),
            ("pkg/auth/**", "pkg/**"),
            ("pkg/*/x.go", "pkg/auth/*"),
            ("**/AGENTS.md", "docs/AGENTS.md"),
            ("**", "a/b/c"),
            ("src/*.rs", "src/lib.rs"),
            ("src/a*.rs", "src/*b.rs"),
            ("pkg/", "pkg/auth/x"),
            ("docs/auth.md", "docs/auth.md"),
        ];
        let no = [
            ("pkg/auth/**", "pkg/billing/**"),
            ("pkg/auth/**", "pkgx/auth"),
            ("src/*.rs", "src/a/b.rs"),
            ("src/a*.rs", "src/b*.go"),
            ("docs/auth.md", "docs/auth.mdx"),
            ("pkg/", "pk"),
        ];
        for (a, b) in yes {
            assert!(glob_overlap(a, b) && glob_overlap(b, a), "{a} {b}");
        }
        for (a, b) in no {
            assert!(!glob_overlap(a, b) && !glob_overlap(b, a), "{a} {b}");
        }
        assert_eq!(literal_prefix("pkg/auth/**"), "pkg/auth");
        assert_eq!(literal_prefix("**"), "");
        assert_eq!(literal_prefix("*/x"), "");
        assert_eq!(literal_prefix("docs/auth.md"), "docs/auth.md");
    }

    /// Backtracking tried every way to split both segment lists between their `**`s; the table takes 13 rows of 31.
    #[test]
    fn many_double_stars_against_globs_that_do_not_overlap_finish() {
        let stars = format!("{}x", "**/".repeat(12));
        let deep = format!("{}y", "a/".repeat(30));
        assert!(!glob_overlap(&stars, &deep) && !glob_overlap(&deep, &stars));
        assert!(!glob_overlap(&stars, &format!("{}y", "**/".repeat(12))));
        assert!(glob_overlap(&stars, &format!("{}x", "a/".repeat(30))));
    }

    /// Backtracking tried about C(42, 12) splits here; the table takes 13 rows of 31.
    #[test]
    fn many_double_stars_against_a_deep_path_that_does_not_match_finish() {
        let glob = format!("{}x", "**/".repeat(12));
        let path = vec!["a"; 30].join("/");
        assert!(!glob_match(&glob, &path));
        assert!(glob_match(&glob, &format!("{path}/x")));
    }
}
