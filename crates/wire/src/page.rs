//! Pagination (`design/SHARED.md`, "Cross-cutting conventions"; `research/wire-census.md` #16): `per_page` clamps to
//! 100 silently, `page` starts at 1, and `link` headers name `/repositories/{id}/...`, so a renamed repository's next
//! page still resolves. GraphQL connection cursors are opaque positions.

use crate::ids::{b64url, unb64url};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Page {
    pub per_page: usize,
    pub page: usize,
}

impl Page {
    pub const DEFAULT: Page = Page { per_page: 30, page: 1 };

    /// From a query string. A missing, zero, or unparsable value takes its default, as GitHub does.
    pub fn parse(query: &str) -> Page {
        let get = |k: &str| {
            query
                .split('&')
                .filter_map(|kv| kv.split_once('='))
                .find(|(a, _)| *a == k)
                .and_then(|(_, v)| v.parse::<usize>().ok())
                .filter(|v| *v > 0)
        };
        Page { per_page: get("per_page").unwrap_or(30).min(100), page: get("page").unwrap_or(1) }
    }

    pub fn skip(&self) -> usize {
        (self.page - 1).saturating_mul(self.per_page)
    }
}

/// The `link` header for `page` of a list at `path` under `/repositories/{repo}`, keeping the request's other query
/// parameters. `more` says a next page exists; `last` is the last page's number when the list knows its size.
pub fn link(
    api: &str,
    repo: i64,
    path: &str,
    query: &[(&str, &str)],
    page: Page,
    more: bool,
    last: Option<usize>,
) -> Option<String> {
    let url = |n: usize| {
        let mut q: Vec<String> =
            query.iter().filter(|(k, _)| *k != "page" && *k != "per_page").map(|(k, v)| format!("{k}={v}")).collect();
        q.push(format!("per_page={}", page.per_page));
        q.push(format!("page={n}"));
        format!("<{api}/repositories/{repo}{path}?{}>", q.join("&"))
    };
    let mut rels = Vec::new();
    if page.page > 1 {
        rels.push(format!("{}; rel=\"prev\"", url(page.page - 1)));
    }
    if more {
        rels.push(format!("{}; rel=\"next\"", url(page.page + 1)));
    }
    if let Some(l) = last.filter(|l| *l > page.page) {
        rels.push(format!("{}; rel=\"last\"", url(l)));
    }
    if page.page > 1 {
        rels.push(format!("{}; rel=\"first\"", url(1)));
    }
    (!rels.is_empty()).then(|| rels.join(", "))
}

/// The repository a REST path names: `/repos/{owner}/{repo}/...` by name, or `/repositories/{id}/...` by id, which
/// is the form every `link` header above writes, so a client following `rel="next"` reaches the same route.
#[derive(Debug, PartialEq, Eq)]
pub enum RepoPath<'a> {
    Name { owner: &'a str, name: &'a str },
    Id(i64),
}

/// A REST path split into the repository it names and the segments after it, or `None` when it names none.
pub fn repo_path(p: &str) -> Option<(RepoPath<'_>, Vec<&str>)> {
    if let Some(rest) = p.strip_prefix("/repos/") {
        let mut segs = rest.split('/');
        let (owner, name) = (segs.next()?, segs.next()?);
        return Some((RepoPath::Name { owner, name }, segs.collect()));
    }
    let mut segs = p.strip_prefix("/repositories/")?.split('/');
    let id = segs.next()?.parse().ok()?;
    Some((RepoPath::Id(id), segs.collect()))
}

/// An opaque connection cursor for position `n`.
pub fn cursor(n: i64) -> String {
    b64url(format!("cursor:{n}").as_bytes())
}

pub fn parse_cursor(s: &str) -> Option<i64> {
    String::from_utf8(unb64url(s)?).ok()?.strip_prefix("cursor:")?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_headers_next_page_names_a_routable_repository_path() {
        let l =
            link("https://h/api/v3", 196641, "/commits/ab/statuses", &[], Page::parse("page=1"), true, None).unwrap();
        let url = l.split('>').next().unwrap().trim_start_matches('<');
        let path = url.strip_prefix("https://h/api/v3").unwrap().split('?').next().unwrap();
        assert_eq!(repo_path(path), Some((RepoPath::Id(196641), vec!["commits", "ab", "statuses"])));
        assert_eq!(repo_path("/repos/o/r/issues"), Some((RepoPath::Name { owner: "o", name: "r" }, vec!["issues"])));
        assert_eq!(repo_path("/repos/o"), None);
        assert_eq!(repo_path("/repositories/x/issues"), None);
        assert_eq!(repo_path("/meta"), None);
    }

    #[test]
    fn per_page_clamps_silently_and_bad_values_take_defaults() {
        assert_eq!(Page::parse(""), Page { per_page: 30, page: 1 });
        assert_eq!(Page::parse("per_page=1000&page=3"), Page { per_page: 100, page: 3 });
        assert_eq!(Page::parse("per_page=0&page=0"), Page { per_page: 30, page: 1 });
        assert_eq!(Page::parse("per_page=x"), Page::DEFAULT);
        assert_eq!(Page::parse("page=2&per_page=10").skip(), 10);
    }

    #[test]
    fn links_name_the_repository_id_and_keep_other_parameters() {
        let p = Page { per_page: 2, page: 2 };
        let l = link("https://h/api/v3", 7, "/releases", &[("state", "all"), ("page", "2")], p, true, Some(4)).unwrap();
        assert_eq!(
            l,
            "<https://h/api/v3/repositories/7/releases?state=all&per_page=2&page=1>; rel=\"prev\", \
             <https://h/api/v3/repositories/7/releases?state=all&per_page=2&page=3>; rel=\"next\", \
             <https://h/api/v3/repositories/7/releases?state=all&per_page=2&page=4>; rel=\"last\", \
             <https://h/api/v3/repositories/7/releases?state=all&per_page=2&page=1>; rel=\"first\""
        );
        assert_eq!(link("a", 1, "/x", &[], Page::DEFAULT, false, Some(1)), None, "one page has no link");
    }

    #[test]
    fn cursors_round_trip() {
        assert_eq!(parse_cursor(&cursor(42)), Some(42));
        assert_eq!(parse_cursor("nope"), None);
    }
}
