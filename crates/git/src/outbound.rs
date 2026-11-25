//! The controller's outbound router (`design/AGENT-LAYER.md`, Shape, "Controller": `outbound` and `Allowed`; journey A3
//! step 5; journey A4; `build/BUILD-PLAN.md`, AM3-WP2), pure. Ported from the M0-WP14 prototype
//! (`tests/m0/src/wp14/router.ts`), which L0-02 ran hosted: 0 of 800 containment attempts succeeded
//! (`tests/m0/results/m0-wp14.md`).
//!
//! Every HTTP and HTTPS request a task's Sandbox sends arrives here, because the controller registers one `*`
//! intercept on a container started with `enableInternet: false`. `outbound` either refuses it or says how to forward
//! it: the URL without userinfo, the headers without any credential the Sandbox supplied, which credential the host
//! attaches, and the body to send. The tokens and the model key never enter this module, so they cannot reach the
//! Sandbox through a refusal's text either. A request no `Allowed` arm matches is refused.
//!
//! - **Every arm:** plain HTTP, a port other than 443, `CONNECT`, and a percent-encoded, dot-segment, or empty-segment
//!   path are refused first, because the router and the upstream could read them as different resources.
//! - **`GraftyardHost`:** smart HTTP on the base and the push target, and REST under `/api/v3/repos/{base or push
//!   target}/`, matched case-insensitively. The push target is the workspace fork in fork mode and the base in-repo. It
//!   attaches the `graftyard-workspace` token on the push target and the `graftyard-agent` token on the base, refuses
//!   receive-pack anywhere else, parses the receive-pack command list and refuses a ref outside the task's refs
//!   (`task::run_refs`), and refuses any `X-HTTP-Method-Override` and every status or check-run write. GraphQL, gists,
//!   uploads, and every other path are refused.
//! - **`ModelInference`:** the one inference path, `POST` only. The JSON body is parsed, and `mcp_servers`, a tool
//!   whose `type` is not a client tool (`function`, `custom`, or none), and the file and batch endpoints are refused.
//!   The body forwarded is the one parsed here, re-serialized, never the Sandbox's bytes.
//! - **`Registry`:** `GET` with no query string. On a Go module proxy every path must name a listed module exactly,
//!   after the proxy protocol's `!x` case escape is decoded. (The M0-WP14 stand-in served Go under `/go/`.)

use std::io::Read;

use serde_json::Value;

use crate::agent::RefPattern;
use crate::controller::{RepoRef, Task, Workspace};
use crate::pkt::{Pkt, Reader};
use crate::task::run_refs;

/// One way out of the Sandbox.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Allowed {
    /// Smart HTTP and REST on the task's base and its push target, on the install's host.
    GraftyardHost { host: Box<str>, base: RepoRef, fork: Option<RepoRef> },
    /// The model's one inference path.
    ModelInference { host: Box<str>, path: Box<str> },
    /// A registry that never fetches upstream on demand. `go_modules` is `Some` for a Go module proxy: every path must
    /// then be a proxy path (`/<module>/@...`) of a listed module.
    Registry { host: Box<str>, go_modules: Option<Vec<Box<str>>> },
}

/// A task's arms and the refs it may push.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Route {
    pub arms: Vec<Allowed>,
    pub refs: RefPattern,
}

impl Route {
    /// The route of a Graftyard-run task: the install's host with the task's base and fork, then `others`. `None` for a
    /// task with no host-owned refs (an external session).
    pub fn of(task: &Task, host: &str, others: Vec<Allowed>) -> Option<Route> {
        let fork = match &task.workspace {
            Workspace::Fork(f) => Some(f.clone()),
            Workspace::InRepo => None,
        };
        let mut arms = vec![Allowed::GraftyardHost { host: host.into(), base: task.base.clone(), fork }];
        arms.extend(others);
        Some(Route { arms, refs: run_refs(&task.id)? })
    }
}

/// A request as the Sandbox sent it. Header names are lowercase.
#[derive(Clone, Copy, Debug)]
pub struct OutReq<'a> {
    pub method: &'a str,
    pub url: &'a str,
    pub headers: &'a [(String, String)],
    pub body: &'a [u8],
}

/// The credential the host attaches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cred {
    /// `graftyard-agent`'s installation token for the base, as `Authorization: Basic x-access-token:<token>` on git
    /// and `Bearer` on REST.
    Agent(Scheme),
    /// `graftyard-workspace`'s installation token for the push target.
    Workspace(Scheme),
    /// The model key, as `Bearer`.
    ModelKey,
    /// Nothing: a registry.
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scheme {
    Basic,
    Bearer,
}

/// The arm a forwarded request matched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arm {
    GraftyardHost,
    ModelInference,
    Registry,
}

/// How to forward a request. The host sends it with `redirect: "manual"`, so a redirect goes back to the Sandbox and
/// its next request comes through here again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Forward {
    pub arm: Arm,
    /// The request's URL without userinfo or fragment.
    pub url: String,
    /// The Sandbox's headers without `Authorization`, `Cookie`, `Host`, `Proxy-Authorization`, `X-Forwarded-Host`, or
    /// `Forwarded`, and, for a replaced body, without its length and encodings.
    pub headers: Vec<(String, String)>,
    pub cred: Cred,
    /// `Some` replaces the Sandbox's body: the model body re-serialized.
    pub body: Option<Vec<u8>>,
}

/// A refusal: the rule's name, the text the Sandbox sees, and the status.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refused {
    pub rule: &'static str,
    pub text: String,
    pub status: u16,
}

fn refuse(rule: &'static str, text: impl Into<String>) -> Refused {
    Refused { rule, text: text.into(), status: 403 }
}

/// The headers the Sandbox may not set on a forwarded request.
const DROPPED: [&str; 6] = ["authorization", "cookie", "host", "proxy-authorization", "x-forwarded-host", "forwarded"];
/// File and batch endpoints on the model host.
const FILE_BATCH: [&str; 7] =
    ["/files", "/batches", "/uploads", "/fine_tuning", "/assistants", "/vector_stores", "/messages/batches"];

struct Url<'a> {
    https: bool,
    host: String,
    port: Option<&'a str>,
    path: &'a str,
    query: Option<&'a str>,
}

/// Splits `https://[userinfo@]host[:port]/path[?query][#fragment]`. `None` for anything else.
fn parse_url(s: &str) -> Option<Url<'_>> {
    let (https, rest) = match s.split_once("://") {
        Some((sc, rest)) if sc.eq_ignore_ascii_case("https") => (true, rest),
        Some((sc, rest)) if sc.eq_ignore_ascii_case("http") => (false, rest),
        _ => return None,
    };
    let rest = rest.split('#').next().unwrap_or_default();
    let end = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, rest) = rest.split_at(end);
    let hostport = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let (host, port) = match hostport.strip_prefix('[') {
        Some(v6) => {
            let (h, after) = v6.split_once(']')?;
            (h, after.strip_prefix(':'))
        }
        None => match hostport.rsplit_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (hostport, None),
        },
    };
    let (path, query) = match rest.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (rest, None),
    };
    let host = host.to_ascii_lowercase();
    Some(Url { https, host: host.strip_suffix('.').unwrap_or(&host).to_owned(), port, path, query })
}

/// Decides one request (see the module doc).
pub fn outbound(req: OutReq<'_>, route: &Route) -> Result<Forward, Refused> {
    let Some(url) = parse_url(req.url) else { return Err(refuse("Url", "the URL could not be parsed")) };
    if !url.https {
        return Err(refuse("PlainHttp", "plain HTTP is refused"));
    }
    if url.port.is_some_and(|p| p != "443") {
        return Err(refuse("Port", format!("port {} is refused", url.port.unwrap_or_default())));
    }
    if req.method.eq_ignore_ascii_case("CONNECT") {
        return Err(refuse("Connect", "CONNECT is refused"));
    }
    let p = url.path;
    if p.contains('%') || p.contains("//") || p.split('/').any(|s| s == "." || s == "..") {
        return Err(refuse("PathEncoding", "encoded, dot-segment, or empty-segment paths are refused"));
    }
    let arm = route.arms.iter().find(|a| match a {
        Allowed::GraftyardHost { host, .. } | Allowed::ModelInference { host, .. } | Allowed::Registry { host, .. } => {
            host.eq_ignore_ascii_case(&url.host)
        }
    });
    let Some(arm) = arm else { return Err(refuse("UnlistedHost", format!("{} is on no Allowed arm", url.host))) };
    let clean = format!("https://{}{}{}", url.host, p, url.query.map(|q| format!("?{q}")).unwrap_or_default());
    let headers = |replaced: bool| -> Vec<(String, String)> {
        req.headers
            .iter()
            .filter(|(k, _)| !DROPPED.contains(&k.as_str()))
            .filter(|(k, _)| {
                !replaced || !matches!(k.as_str(), "content-length" | "content-encoding" | "transfer-encoding")
            })
            .cloned()
            .collect()
    };
    let fwd = |arm, cred, body: Option<Vec<u8>>| Forward {
        arm,
        url: clean.clone(),
        headers: headers(body.is_some()),
        cred,
        body,
    };
    match arm {
        Allowed::Registry { go_modules, .. } => {
            if !req.method.eq_ignore_ascii_case("GET") || url.query.is_some() {
                return Err(refuse("RegistryMethod", "the registry takes a plain GET only"));
            }
            if let Some(known) = go_modules {
                let module = p
                    .strip_prefix('/')
                    .and_then(go_module)
                    .ok_or_else(|| refuse("RegistryPath", format!("{p} is not a Go proxy path")))?;
                if !known.iter().any(|k| **k == *module) {
                    return Err(refuse("UnknownGoModule", format!("Go module {module} is not in the mirror")));
                }
            }
            Ok(fwd(Arm::Registry, Cred::None, None))
        }
        Allowed::ModelInference { path, .. } => {
            if FILE_BATCH.iter().any(|s| p.starts_with(s) || p.strip_prefix("/v1").is_some_and(|q| q.starts_with(s))) {
                return Err(refuse("ModelFileBatch", format!("file and batch endpoints are refused ({p})")));
            }
            if p != &**path {
                return Err(refuse("ModelPath", format!("only {path} is allowed on the model host")));
            }
            if !req.method.eq_ignore_ascii_case("POST") {
                return Err(refuse("ModelMethod", "the inference path takes POST only"));
            }
            let Ok(body) = serde_json::from_slice::<Value>(req.body) else {
                return Err(Refused { rule: "ModelBody", text: "the body must be JSON".into(), status: 400 });
            };
            if let Some(why) = model_refusal(&body) {
                return Err(refuse("ModelPayload", why));
            }
            let mut f = fwd(Arm::ModelInference, Cred::ModelKey, Some(serde_json::to_vec(&body).expect("a Value")));
            f.headers.retain(|(k, _)| k != "content-type");
            f.headers.push(("content-type".into(), "application/json".into()));
            Ok(f)
        }
        Allowed::GraftyardHost { base, fork, .. } => {
            graftyard_host(req, p, url.query, base, fork.as_ref(), &route.refs)
                .map(|(cred, body)| fwd(Arm::GraftyardHost, cred, body))
        }
    }
}

/// The `GraftyardHost` arm: the credential to attach, and never a replaced body.
fn graftyard_host(
    req: OutReq<'_>,
    p: &str,
    query: Option<&str>,
    base: &RepoRef,
    fork: Option<&RepoRef>,
    refs: &RefPattern,
) -> Result<(Cred, Option<Vec<u8>>), Refused> {
    if req.headers.iter().any(|(k, _)| k == "x-http-method-override") {
        return Err(refuse("MethodOverride", "X-HTTP-Method-Override is refused"));
    }
    // `/{owner}/{name}.git[/...]`: the first `.git` that ends the second segment.
    let git = p.strip_prefix('/').and_then(|s| {
        let (owner, rest) = s.split_once('/')?;
        let at = rest
            .match_indices(".git")
            .map(|(i, _)| i)
            .find(|&i| !rest[..i].contains('/') && matches!(rest.as_bytes().get(i + 4), None | Some(b'/')))?;
        Some((owner, &rest[..at], &rest[at + 4..]))
    });
    let rest = p.strip_prefix("/api/v3/repos/").and_then(|s| {
        let mut it = s.splitn(3, '/');
        let (owner, name) = (it.next()?, it.next()?);
        Some((owner, name, it.next().map(|t| format!("/{t}")).unwrap_or_default()))
    });
    let (owner, name) = match (&git, &rest) {
        (Some((o, n, _)), _) => (*o, *n),
        (None, Some((o, n, _))) => (*o, *n),
        (None, None) => {
            let lp = p.to_ascii_lowercase();
            let under = |s: &str| {
                [s.to_owned(), format!("/api{s}"), format!("/api/v3{s}")].iter().any(|x| lp.starts_with(x.as_str()))
            };
            return Err(if under("/graphql") {
                refuse("GraphQL", "GraphQL is refused")
            } else if under("/gists") {
                refuse("Gist", "gists are refused")
            } else if under("/uploads") {
                refuse("Upload", "uploads are refused")
            } else {
                refuse("OwnerPath", format!("{p} is not git or REST on the task's repositories"))
            });
        }
    };
    let is = |r: &RepoRef| r.owner.as_str().eq_ignore_ascii_case(owner) && r.name.as_str().eq_ignore_ascii_case(name);
    let target = fork.unwrap_or(base);
    let cred = |scheme| if is(target) { Cred::Workspace(scheme) } else { Cred::Agent(scheme) };
    if !is(base) && !is(target) {
        return Err(refuse("OtherRepo", format!("{owner}/{name} is not the task's base or push target")));
    }
    if let Some((_, _, tail)) = git {
        let receive = tail == "/git-receive-pack"
            || tail == "/info/refs" && query.is_some_and(|q| q.split('&').any(|kv| kv == "service=git-receive-pack"));
        if receive {
            if !is(target) {
                return Err(refuse("ReceivePackBase", format!("receive-pack is refused on {owner}/{name}")));
            }
            if tail == "/git-receive-pack" {
                let gz =
                    req.headers.iter().any(|(k, v)| k == "content-encoding" && v.to_ascii_lowercase().contains("gzip"));
                let Some(names) = receive_refs(req.body, gz) else {
                    return Err(Refused {
                        rule: "ReceivePackParse",
                        text: "the receive-pack request could not be parsed".into(),
                        status: 400,
                    });
                };
                if let Some(out) = names.iter().find(|r| !refs.matches(r.as_bytes())) {
                    return Err(refuse("OutsidePrefix", format!("{out} is outside {}", refs.as_str())));
                }
                if names.is_empty() {
                    return Err(refuse("OutsidePrefix", "a push with no ref is refused"));
                }
            }
        } else if !(tail == "/info/refs"
            || tail == "/git-upload-pack"
            || tail == "/HEAD"
            || tail.starts_with("/objects/"))
        {
            return Err(refuse("GitPath", format!("git path {tail} is refused")));
        }
        return Ok((cred(Scheme::Basic), None));
    }
    let sub = rest.map(|(_, _, t)| t.to_ascii_lowercase()).unwrap_or_default();
    let write = !(req.method.eq_ignore_ascii_case("GET") || req.method.eq_ignore_ascii_case("HEAD"));
    let segs: Vec<&str> = sub.split('/').skip(1).collect();
    let checks = |s: &str| matches!(s, "statuses" | "check-runs" | "check-suites");
    if write
        && (segs.first().is_some_and(|s| checks(s))
            || segs.first() == Some(&"commits") && segs.get(2).is_some_and(|s| checks(s)))
    {
        return Err(refuse("StatusOrCheckRun", format!("status and check-run writes are refused ({sub})")));
    }
    Ok((cred(Scheme::Bearer), None))
}

/// Why a model body is refused, or `None`.
pub fn model_refusal(body: &Value) -> Option<String> {
    let Value::Object(b) = body else { return Some("the body must be a JSON object".into()) };
    if b.contains_key("mcp_servers") {
        return Some("mcp_servers is refused".into());
    }
    for key in ["tools", "builtin_tools"] {
        let Some(tools) = b.get(key) else { continue };
        let Value::Array(tools) = tools else { return Some(format!("{key} must be an array")) };
        for t in tools {
            let Value::Object(t) = t else { return Some(format!("each of {key} must be an object")) };
            match t.get("type") {
                None => {}
                Some(Value::String(ty)) if ty == "function" || ty == "custom" => {}
                Some(ty) => return Some(format!("tool type {ty} is not a client tool")),
            }
        }
    }
    None
}

/// The ref names of a receive-pack request's command list, up to its flush; `None` when it does not parse.
pub fn receive_refs(body: &[u8], gzip: bool) -> Option<Vec<String>> {
    let mut raw = Vec::new();
    let buf = if gzip {
        flate2::read::GzDecoder::new(body).read_to_end(&mut raw).ok()?;
        &raw[..]
    } else {
        body
    };
    let mut r = Reader::new(buf);
    let mut names = Vec::new();
    loop {
        let line = match r.next_pkt().ok()?? {
            Pkt::Flush => return Some(names),
            p @ Pkt::Data(_) => p.text()?,
            Pkt::Delim | Pkt::End => return None,
        };
        let cmd = line.split(|&b| b == 0).next()?;
        if cmd.starts_with(b"shallow ") {
            continue;
        }
        let mut parts = cmd.splitn(3, |&b| b == b' ');
        let (_old, _new, name) = (parts.next()?, parts.next()?, parts.next()?);
        names.push(String::from_utf8(name.to_vec()).ok()?);
    }
}

/// A Go proxy path's module (`<module>/@latest`, `<module>/@v/list`, `<module>/@v/v<version>.{info,mod,zip}`), its
/// `!x` escapes decoded; `None` for any other path or an invalid escape (https://go.dev/ref/mod#goproxy-protocol).
pub fn go_module(path: &str) -> Option<String> {
    let (m, tail) = path.split_once("/@")?;
    let ok = tail == "latest"
        || tail == "v/list"
        || tail.strip_prefix("v/v").is_some_and(|v| {
            v.starts_with(|c: char| c.is_ascii_digit())
                && !v.contains('/')
                && [".info", ".mod", ".zip"].iter().any(|e| v.ends_with(e))
        });
    if !ok || m.is_empty() {
        return None;
    }
    let mut out = String::new();
    let mut it = m.chars();
    while let Some(c) = it.next() {
        match c {
            'A'..='Z' => return None,
            '!' => out.push(it.next().filter(char::is_ascii_lowercase)?.to_ascii_uppercase()),
            c => out.push(c),
        }
    }
    Some(out)
}
