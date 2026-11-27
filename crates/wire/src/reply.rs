//! What every response carries (`design/SHARED.md`, "Cross-cutting conventions"; `research/wire-census.md` #11 to #22):
//! the request id and enterprise version, REST's media type and selected API version, weak ETags with `304`, error
//! bodies, and the `User-Agent` and API-version admission checks.

use graftyard_types::{Index, RequestId};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use crate::rate::RateLimits;

pub const ENTERPRISE_VERSION: &str = "3.22.0";
const DOCS: &str = "https://docs.github.com/enterprise-server@3.22";

/// `X-GitHub-Request-Id` in GitHub's colon-separated hex shape, carrying the whole request id.
pub fn request_id_header(rid: RequestId) -> String {
    let h = format!("{:032X}", rid.0.0);
    format!("{}:{}:{}:{}:{}", &h[..4], &h[4..8], &h[8..16], &h[16..24], &h[24..])
}

/// Which family of surface rendered a reply: REST-shaped surfaces get REST's headers, every surface the request id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Surface {
    /// `/api/v3` and `/api/uploads`.
    Rest,
    Graphql,
    Git,
    Lfs,
    /// Raw content, archives, release downloads, and LFS transfer bytes.
    Content,
}

impl Surface {
    fn versioned(self) -> bool {
        matches!(self, Surface::Rest | Surface::Graphql)
    }
}

/// `X-GitHub-Api-Version`. GHES 3.22 supports both (`research/wire-census.md` #12).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApiVersion {
    V2022_11_28,
    V2026_03_10,
}

impl ApiVersion {
    pub fn parse(v: Option<&str>) -> Option<ApiVersion> {
        match v.map(str::trim) {
            None | Some("2022-11-28") => Some(ApiVersion::V2022_11_28),
            Some("2026-03-10") => Some(ApiVersion::V2026_03_10),
            Some(_) => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ApiVersion::V2022_11_28 => "2022-11-28",
            ApiVersion::V2026_03_10 => "2026-03-10",
        }
    }
}

/// The custom media type a REST request asks for: `application/vnd.github[.v3][.PARAM][+json]`, where `PARAM` is
/// `raw`, `html`, `text`, `full`, `diff`, `patch`, `sha`, or `object` (`research/wire-census.md` #14). Any other
/// `Accept`, `application/octet-stream` included, is the default.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct MediaType {
    pub param: Option<String>,
}

impl MediaType {
    pub fn parse(accept: Option<&str>) -> MediaType {
        let param = accept.unwrap_or("").split(',').find_map(|t| {
            let t = t.split(';').next()?.trim();
            let rest = t.strip_prefix("application/vnd.github")?;
            let rest = rest.strip_suffix("+json").unwrap_or(rest);
            let rest = rest.strip_prefix(".v3").unwrap_or(rest);
            let p = rest.strip_prefix('.')?;
            ["raw", "html", "text", "full", "diff", "patch", "sha", "object"].contains(&p).then(|| p.to_owned())
        });
        MediaType { param }
    }

    /// `x-github-media-type`.
    pub fn header(&self) -> String {
        match &self.param {
            None => "github.v3; format=json".into(),
            Some(p) => format!("github.v3; param={p}; format=json"),
        }
    }
}

/// A request's headers that the wire layer reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Incoming<'a> {
    pub user_agent: Option<&'a str>,
    pub api_version: Option<&'a str>,
    pub accept: Option<&'a str>,
    pub if_none_match: Option<&'a str>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Body {
    Json(Value),
    Bytes(Vec<u8>),
    Empty,
}

/// A handler's answer: its status, body, and the headers only it knows (`link`, `etag`, `location`,
/// `content-type` for bytes). `finish` adds the rest.
#[derive(Clone, Debug, PartialEq)]
pub struct Reply {
    pub status: u16,
    pub body: Body,
    pub headers: Vec<(&'static str, String)>,
}

impl Reply {
    pub fn json(status: u16, body: Value) -> Reply {
        Reply { status, body: Body::Json(body), headers: Vec::new() }
    }

    pub fn bytes(status: u16, content_type: &str, body: Vec<u8>) -> Reply {
        Reply { status, body: Body::Bytes(body), headers: vec![("content-type", content_type.to_owned())] }
    }

    pub fn empty(status: u16) -> Reply {
        Reply { status, body: Body::Empty, headers: Vec::new() }
    }

    pub fn error(status: u16, message: &str, doc: &str) -> Reply {
        Reply::json(status, basic_error(status, message, doc))
    }

    /// Agent admission is full (`graftyard_raft::admit`): 429 with `retry-after` and GitHub's secondary rate limit
    /// message (`design/DESIGN.md`, Shape, "Agent load and agent safety").
    pub fn secondary_rate_limit(retry_after_s: u32) -> Reply {
        Reply::error(
            429,
            "You have exceeded a secondary rate limit. Please wait a few minutes before you try again.",
            "rest/using-the-rest-api/rate-limits-for-the-rest-api#about-secondary-rate-limits",
        )
        .with("retry-after", retry_after_s.to_string())
    }

    pub fn not_found() -> Reply {
        Reply::error(404, "Not Found", "rest")
    }

    pub fn with(mut self, name: &'static str, value: impl Into<String>) -> Reply {
        self.headers.retain(|(n, _)| *n != name);
        self.headers.push((name, value.into()));
        self
    }

    /// The same reply with its error body's `documentation_url` naming `doc`, for a refusal a shared renderer built
    /// for another route. A reply whose page is the API's root (`rest`: 401, 404, 503) keeps it, as does a body with
    /// no `documentation_url`.
    pub fn with_doc(mut self, doc: &str) -> Reply {
        let root = format!("{DOCS}/rest");
        if let Body::Json(Value::Object(o)) = &mut self.body
            && let Some(Value::String(u)) = o.get_mut("documentation_url")
            && *u != root
        {
            *u = format!("{DOCS}/{doc}");
        }
        self
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    /// The JSON body, or `Null` for any other body.
    pub fn value(&self) -> &Value {
        match &self.body {
            Body::Json(v) => v,
            _ => &Value::Null,
        }
    }
}

/// GitHub's `basic-error`, with `status` a string.
pub fn basic_error(status: u16, message: &str, doc: &str) -> Value {
    json!({ "message": message, "documentation_url": format!("{DOCS}/{doc}"), "status": status.to_string() })
}

/// One entry of `validation-error`'s `errors`. `code` is `missing`, `missing_field`, `invalid`, `already_exists`,
/// `unprocessable`, or `custom` (`research/wire-census.md` #20).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldError {
    pub resource: &'static str,
    pub field: &'static str,
    pub code: &'static str,
}

pub fn validation_error(message: &str, doc: &str, errors: &[FieldError]) -> Value {
    let errors: Vec<Value> =
        errors.iter().map(|e| json!({ "resource": e.resource, "field": e.field, "code": e.code })).collect();
    json!({ "message": message, "errors": errors, "documentation_url": format!("{DOCS}/{doc}"), "status": "422" })
}

/// A weak ETag that derives from the resource and the index of the last entry that changed it, so a write to another
/// resource of the same chain leaves it valid.
pub fn etag(resource: &str, index: Index) -> String {
    let mut h = Sha256::new();
    h.update(resource.as_bytes());
    h.update(index.0.to_be_bytes());
    let d = h.finalize();
    let hex: String = d[..16].iter().map(|b| format!("{b:02x}")).collect();
    format!("W/\"{hex}\"")
}

/// Whether `If-None-Match` names `tag`, by the weak comparison RFC 9110 gives for `If-None-Match`.
pub fn not_modified(if_none_match: Option<&str>, tag: &str) -> bool {
    let bare = |t: &str| t.trim().trim_start_matches("W/").to_owned();
    if_none_match.is_some_and(|h| h.trim() == "*" || h.split(',').any(|t| bare(t) == bare(tag)))
}

/// Admits a request to `surface`: a `User-Agent` is required everywhere (403 otherwise), and an unknown
/// `X-GitHub-Api-Version` is 400 on REST and GraphQL.
pub fn serve(i: &Incoming<'_>, surface: Surface) -> Result<ApiVersion, Reply> {
    if i.user_agent.is_none_or(|u| u.trim().is_empty()) {
        let msg = "Request forbidden by administrative rules. Please make sure your request has a User-Agent header \
                   (https://docs.github.com/enterprise-server@3.22/rest/overview/resources-in-the-rest-api#user-agent-required). \
                   Check https://developer.github.com for other possible causes.\n";
        return Err(Reply::bytes(403, "text/plain; charset=utf-8", msg.as_bytes().to_vec()));
    }
    match ApiVersion::parse(i.api_version) {
        Some(v) => Ok(v),
        None if !surface.versioned() => Ok(ApiVersion::V2022_11_28),
        None => Err(Reply::error(
            400,
            &format!("Invalid API version: {}", i.api_version.unwrap_or("")),
            "rest/about-the-rest-api/api-versions",
        )),
    }
}

/// What the edge knows of the request beyond its headers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Meta {
    pub rid: RequestId,
    pub version: ApiVersion,
    pub rate: RateLimits,
}

/// Adds every cross-cutting header to `r`, and turns a `200` whose ETag the client already holds into a bodyless
/// `304`. A `304` costs one fresh read and does not count against the rate limit; the caller asks
/// `counts_against_limit`.
pub fn finish(i: &Incoming<'_>, mut r: Reply, surface: Surface, m: &Meta) -> Reply {
    if r.status == 200 && r.header("etag").is_some_and(|t| not_modified(i.if_none_match, t)) {
        r.status = 304;
        r.body = Body::Empty;
        r.headers.retain(|(n, _)| matches!(*n, "etag" | "last-modified" | "cache-control" | "link"));
    }
    if matches!(r.body, Body::Json(_)) && r.header("content-type").is_none() {
        r = r.with("content-type", "application/json; charset=utf-8");
    }
    r = r.with("x-github-request-id", request_id_header(m.rid)).with("x-github-enterprise-version", ENTERPRISE_VERSION);
    if surface == Surface::Rest {
        r = r
            .with("x-github-media-type", MediaType::parse(i.accept).header())
            .with("x-github-api-version-selected", m.version.as_str());
    }
    if surface.versioned() {
        let resource = if surface == Surface::Graphql { "graphql" } else { "core" };
        for (k, v) in m.rate.headers(resource) {
            r = r.with(k, v);
        }
    }
    r
}

pub fn counts_against_limit(r: &Reply) -> bool {
    r.status != 304
}

#[cfg(test)]
mod tests {
    /// Agent admission's REST shape: 429, `retry-after`, and GitHub's secondary rate limit message.
    #[test]
    fn secondary_rate_limit_is_a_429_with_retry_after() {
        let r = super::Reply::secondary_rate_limit(12);
        assert_eq!(r.status, 429);
        assert_eq!(r.header("retry-after"), Some("12"));
        let super::Body::Json(b) = &r.body else { panic!("json") };
        assert!(b["message"].as_str().unwrap().contains("secondary rate limit"), "{b}");
        assert_eq!(b["status"], "429");
    }

    use graftyard_types::Ulid;

    use super::*;

    fn meta() -> Meta {
        Meta {
            rid: RequestId(Ulid(0x0123_4567_89ab_cdef_0011_2233_4455_6677)),
            version: ApiVersion::V2022_11_28,
            rate: RateLimits::Off,
        }
    }

    #[test]
    fn the_request_id_header_has_githubs_shape_and_the_whole_id() {
        let h = request_id_header(meta().rid);
        assert_eq!(h, "0123:4567:89ABCDEF:00112233:44556677");
        assert_eq!(u128::from_str_radix(&h.replace(':', ""), 16).unwrap(), meta().rid.0.0);
    }

    #[test]
    fn every_surface_gets_the_request_id_and_only_rest_the_media_type() {
        let i = Incoming { user_agent: Some("x"), ..Incoming::default() };
        for s in [Surface::Rest, Surface::Graphql, Surface::Git, Surface::Lfs, Surface::Content] {
            let r = finish(&i, Reply::empty(200), s, &meta());
            assert_eq!(r.header("x-github-request-id"), Some("0123:4567:89ABCDEF:00112233:44556677"));
            assert_eq!(r.header("x-github-enterprise-version"), Some("3.22.0"));
            assert_eq!(r.header("x-github-media-type").is_some(), s == Surface::Rest, "{s:?}");
            assert_eq!(r.header("x-github-api-version-selected").is_some(), s == Surface::Rest);
        }
        let raw = Incoming { user_agent: Some("x"), accept: Some("application/vnd.github.raw+json"), ..i };
        let r = finish(&raw, Reply::json(200, json!({})), Surface::Rest, &meta());
        assert_eq!(r.header("x-github-media-type"), Some("github.v3; param=raw; format=json"));
        assert_eq!(r.header("content-type"), Some("application/json; charset=utf-8"));
    }

    #[test]
    fn no_user_agent_is_403_and_an_unknown_version_400_on_rest_only() {
        assert_eq!(serve(&Incoming::default(), Surface::Git).unwrap_err().status, 403);
        let ua = Incoming { user_agent: Some("gh"), api_version: Some("2099-01-01"), ..Incoming::default() };
        assert_eq!(serve(&ua, Surface::Rest).unwrap_err().status, 400);
        assert_eq!(serve(&ua, Surface::Rest).unwrap_err().value()["status"], "400");
        assert_eq!(serve(&ua, Surface::Lfs), Ok(ApiVersion::V2022_11_28));
        let v26 = Incoming { api_version: Some("2026-03-10"), ..ua };
        assert_eq!(serve(&v26, Surface::Rest), Ok(ApiVersion::V2026_03_10));
        assert_eq!(serve(&Incoming { api_version: None, ..ua }, Surface::Rest), Ok(ApiVersion::V2022_11_28));
    }

    #[test]
    fn a_held_etag_is_a_bodyless_304() {
        let tag = etag("repos/7/releases/1", Index(12));
        assert!(tag.starts_with("W/\""));
        assert_ne!(tag, etag("repos/7/releases/1", Index(13)));
        assert_ne!(tag, etag("repos/7/releases/2", Index(12)));
        let held = tag.trim_start_matches("W/").to_owned();
        let i = Incoming { user_agent: Some("x"), if_none_match: Some(&held), ..Incoming::default() };
        let r = finish(&i, Reply::json(200, json!({"a": 1})).with("etag", tag.clone()), Surface::Rest, &meta());
        assert_eq!((r.status, &r.body), (304, &Body::Empty));
        assert!(!counts_against_limit(&r));
        assert_eq!(r.header("etag"), Some(tag.as_str()));
        let other = Incoming { if_none_match: Some("W/\"x\""), ..i };
        assert_eq!(finish(&other, Reply::json(200, json!({})).with("etag", tag), Surface::Rest, &meta()).status, 200);
    }

    #[test]
    fn error_bodies_have_a_string_status() {
        assert_eq!(basic_error(404, "Not Found", "rest")["status"], "404");
        let v = validation_error(
            "Validation Failed",
            "rest",
            &[FieldError { resource: "Release", field: "tag_name", code: "already_exists" }],
        );
        assert_eq!(v["errors"][0]["code"], "already_exists");
        assert_eq!(v["status"], "422");
    }
}
