//! Rate-limit headers and the `/rate_limit` body (`design/SHARED.md`, "Cross-cutting conventions": "GHES rate limits are
//! off by default in BYOC and self-hosted, and on in managed"; `rest:rate-limit`; `research/wire-census.md` #18 and
//! #19). The counts come from the `RateCounter` objects (`design/DESIGN.md`, Shape, Identity), which are not built yet;
//! this module renders whatever window it is given.

use serde_json::{Value, json};

/// One resource's window as a `RateCounter` reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    pub limit: u64,
    pub used: u64,
    /// Epoch seconds.
    pub reset: u64,
}

impl Window {
    pub fn remaining(&self) -> u64 {
        self.limit.saturating_sub(self.used)
    }
}

/// `None` when limits are off, as on a default GHES: then no response carries `x-ratelimit-*` and `/rate_limit`
/// answers 404 "Rate limiting is not enabled." [doc: GHES 3.22 lists 404 for `GET /rate_limit`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RateLimits {
    Off,
    /// The windows by resource name (`core`, `graphql`, `search`, ...), `core` first.
    On(Vec<(&'static str, Window)>),
}

impl RateLimits {
    /// The headers a response in `resource` carries.
    pub fn headers(&self, resource: &str) -> Vec<(&'static str, String)> {
        let RateLimits::On(w) = self else { return Vec::new() };
        let Some((name, w)) = w.iter().find(|(n, _)| *n == resource).or(w.first()) else { return Vec::new() };
        vec![
            ("x-ratelimit-limit", w.limit.to_string()),
            ("x-ratelimit-remaining", w.remaining().to_string()),
            ("x-ratelimit-reset", w.reset.to_string()),
            ("x-ratelimit-used", w.used.to_string()),
            ("x-ratelimit-resource", (*name).to_owned()),
        ]
    }

    /// `GET /rate_limit`'s status and body. Version `2026-03-10` drops `rate` (`research/wire-census.md` #13); the
    /// pinned GHES 3.22 description, which is the `2022-11-28` shape, requires it.
    pub fn body(&self, v2026: bool) -> (u16, Value) {
        let RateLimits::On(w) = self else {
            return (404, crate::reply::basic_error(404, "Rate limiting is not enabled.", "rest/rate-limit"));
        };
        let one =
            |w: &Window| json!({ "limit": w.limit, "remaining": w.remaining(), "reset": w.reset, "used": w.used });
        let resources: serde_json::Map<String, Value> = w.iter().map(|(n, w)| ((*n).to_owned(), one(w))).collect();
        let mut body = json!({ "resources": resources });
        if !v2026 && let Some((_, core)) = w.first() {
            body["rate"] = one(core);
        }
        (200, body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_means_no_headers_and_a_404() {
        assert!(RateLimits::Off.headers("core").is_empty());
        assert_eq!(RateLimits::Off.body(false).0, 404);
    }

    #[test]
    fn on_renders_githubs_headers_and_drops_rate_in_2026() {
        let r = RateLimits::On(vec![("core", Window { limit: 5000, used: 12, reset: 99 })]);
        assert_eq!(r.headers("core")[1], ("x-ratelimit-remaining", "4988".to_owned()));
        assert_eq!(r.body(false).1["rate"]["remaining"], 4988);
        assert!(r.body(true).1.get("rate").is_none());
    }
}
