//! `graft-mcp`, the MCP authorization server at `mcp.<host>` (`design/AGENT-LAYER.md`, Shape, "graft-mcp"; journey A7;
//! `build/BUILD-PLAN.md`, AM2-WP1). This module holds the types the design names (`Consent`, `McpToken`,
//! `McpRefresh`, `Toolset`, and `WriteScope`, which the host's `Tokens` rows share) and the rules that need no network:
//! sealing, the OAuth scope, PKCE S256, the RFC 9728 and RFC 8414 metadata, and the CIMD checks. `flow` runs A7 over
//! the edge; `worker` is the Workers host.
//!
//! It holds no state of its own. Grant rows live on the user's `Identity` group, behind the host's
//! `/api/graftyard/v1/mcp/grants`, and every token, code, form, and cookie it hands out is sealed with AES-256-GCM under
//! the `MCP_SEAL_KEY` that `graftyard apps init` put in the install's Secrets Store. Each kind is sealed under its own
//! key, derived from the seal key and the kind's prefix, so one kind never opens as another, and an access token
//! (`gymcp_`) has a prefix no `/api/v3` route knows, so the host refuses it with 401.

pub mod flow;
pub mod rpc;
pub mod tools;
#[cfg(target_arch = "wasm32")]
mod worker;

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
pub use graftyard_identity::{Url, WriteScope};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// An `McpToken` lives an hour (A7 step 6).
pub const TOKEN_TTL_MS: u64 = 3_600_000;
/// An `McpRefresh` lives 30 days [guess: M0-WP15's prototype].
pub const REFRESH_TTL_MS: u64 = 30 * 86_400_000;
/// Our authorization code lives a minute [guess: RFC 6749 4.1.2 recommends at most 10 minutes].
pub const CODE_TTL_MS: u64 = 60_000;
/// The consent form and its `__Host-` cookie live 10 minutes [guess: M0-WP15's prototype].
pub const CONSENT_TTL_MS: u64 = 600_000;
/// CIMD fetches: a 5 KB cap and a 5 s timeout (A7 step 3), cached at most a day [guess].
pub const CIMD_MAX_BYTES: usize = 5_120;
pub const CIMD_TIMEOUT_MS: u32 = 5_000;
pub const CIMD_CACHE_MAX_S: u64 = 86_400;
/// Repositories one consent may name [guess: `McpToken.repos` is a `SmallVec<[RepoId; 4]>`].
pub const REPOS_MAX: usize = 4;
/// `/authorize` calls per IP a minute in one isolate; the WAF rule on `mcp.<host>` is the install-wide limit
/// (`design/AGENT-LAYER.md`, Failure modes, graft-mcp noisy-tenant) [guess: M0-WP15's prototype].
pub const AUTHORIZE_PER_MIN: u32 = 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Toolset {
    Context,
    Repos,
    Issues,
    PullRequests,
    Checks,
    Coordination,
    Provenance,
}

impl Toolset {
    pub const ALL: [Toolset; 7] = [
        Toolset::Context,
        Toolset::Repos,
        Toolset::Issues,
        Toolset::PullRequests,
        Toolset::Checks,
        Toolset::Coordination,
        Toolset::Provenance,
    ];

    pub fn wire(self) -> &'static str {
        match self {
            Toolset::Context => "context",
            Toolset::Repos => "repos",
            Toolset::Issues => "issues",
            Toolset::PullRequests => "pull_requests",
            Toolset::Checks => "checks",
            Toolset::Coordination => "coordination",
            Toolset::Provenance => "provenance",
        }
    }
}

/// A set of `Toolset`s, one bit each in `Toolset::ALL`'s order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolsetMask(pub u8);

impl ToolsetMask {
    /// What a client gets when its scope names no toolsets [inference: M0-WP15's default].
    pub const DEFAULT: ToolsetMask = ToolsetMask(0b1111);

    pub fn of(t: Toolset) -> ToolsetMask {
        ToolsetMask(1 << Toolset::ALL.iter().position(|x| *x == t).expect("listed"))
    }

    pub fn has(self, t: Toolset) -> bool {
        self.0 & ToolsetMask::of(t).0 != 0
    }

    pub fn within(self, grant: ToolsetMask) -> bool {
        self.0 & !grant.0 == 0
    }

    pub fn wire(self) -> String {
        Toolset::ALL.iter().filter(|t| self.has(**t)).map(|t| t.wire()).collect::<Vec<_>>().join(",")
    }
}

/// What the consent page shows and the user approves (Shape, "graft-mcp"). `repos` are `owner/name`, all of one owner,
/// as `token/scoped` takes one `target` and its repository names; the host's scoped token row holds their ids.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Consent {
    pub client: Url,
    pub redirect_hosts: Vec<String>,
    pub localhost_only: bool,
    pub repos: Vec<String>,
    pub tools: ToolsetMask,
    pub write: WriteScope,
}

/// The access token a client holds, sealed as `gymcp_…`. `upstream` is the repository- and permission-scoped `ghu_`
/// from `token/scoped`, which only `graft-mcp` sees; `grant` is the host's grant id (`<account>.<n>`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpToken {
    pub aud: Url,
    pub sub: i64,
    pub client: Url,
    pub grant: String,
    pub repos: Vec<String>,
    pub tools: ToolsetMask,
    pub write: WriteScope,
    pub upstream: String,
    pub exp: u64,
}

/// The refresh token, sealed as `gymcr_…`. It seals the narrowing and the grant's `generation`, so every refresh calls
/// `token/scoped` again and an older generation revokes the grant. `upstream` is the `ghr_`; `held` is the unscoped
/// `ghu_` it pairs with, kept until the next refresh has spent the `ghr_`, because the host revokes a pair together.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpRefresh {
    pub sub: i64,
    pub aud: Url,
    pub client: Url,
    pub grant: String,
    pub generation: u32,
    pub repos: Vec<String>,
    pub tools: ToolsetMask,
    pub write: WriteScope,
    pub upstream: String,
    pub held: String,
    pub exp: u64,
}

/// What a sealed string is. Each kind has its own prefix and its own key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sealed {
    Access,
    Refresh,
    Code,
    /// The consent form's hidden field.
    Pending,
    /// The `__Host-graft_mcp` cookie bound to the upstream `state`.
    Cookie,
    /// The settings page's signed-in session.
    Session,
}

impl Sealed {
    pub fn prefix(self) -> &'static str {
        match self {
            Sealed::Access => "gymcp_",
            Sealed::Refresh => "gymcr_",
            Sealed::Code => "gymcc_",
            Sealed::Pending => "gymcf_",
            Sealed::Cookie => "gymck_",
            Sealed::Session => "gymcs_",
        }
    }
}

/// AES-256-GCM under the seal key's slots, newest first: it seals with the first and opens with any, so a rotation
/// keeps old tokens readable until they lapse (Failure modes, graft-mcp bad-config).
pub struct Sealer {
    keys: Vec<[u8; 32]>,
}

impl Sealer {
    pub fn new(keys: Vec<[u8; 32]>) -> Option<Sealer> {
        (!keys.is_empty()).then_some(Sealer { keys })
    }

    /// A slot from Secrets Store: 64 hex characters, as `graftyard apps init` writes it.
    pub fn key_from_hex(s: &str) -> Option<[u8; 32]> {
        let s = s.trim();
        if s.len() != 64 {
            return None;
        }
        let mut k = [0u8; 32];
        for (i, b) in k.iter_mut().enumerate() {
            *b = u8::from_str_radix(s.get(2 * i..2 * i + 2)?, 16).ok()?;
        }
        Some(k)
    }

    fn cipher(key: &[u8; 32], kind: Sealed) -> Aes256Gcm {
        let k: [u8; 32] =
            Sha256::new().chain_update(b"graft-mcp/").chain_update(kind.prefix()).chain_update(key).finalize().into();
        Aes256Gcm::new(&k.into())
    }

    pub fn seal<T: Serialize>(&self, kind: Sealed, v: &T, nonce: [u8; 12]) -> String {
        let plain = serde_json::to_vec(v).expect("sealed values serialize");
        let ct =
            Sealer::cipher(&self.keys[0], kind).encrypt(Nonce::from_slice(&nonce), plain.as_slice()).expect("seal");
        let mut b = nonce.to_vec();
        b.extend(ct);
        format!("{}{}", kind.prefix(), B64.encode(b))
    }

    pub fn open<T: DeserializeOwned>(&self, kind: Sealed, s: &str) -> Option<T> {
        let b = B64.decode(s.strip_prefix(kind.prefix())?).ok()?;
        if b.len() < 12 {
            return None;
        }
        let (nonce, ct) = b.split_at(12);
        let plain =
            self.keys.iter().find_map(|k| Sealer::cipher(k, kind).decrypt(Nonce::from_slice(nonce), ct).ok())?;
        serde_json::from_slice(&plain).ok()
    }
}

/// The OAuth `scope`: space-separated `write:<read|write|write_approve_merge>` and `tools:<a,b>`, plus
/// `offline_access`, which changes nothing. Any other part refuses the whole scope rather than being dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scope {
    pub write: WriteScope,
    pub tools: ToolsetMask,
}

impl Scope {
    pub fn parse(s: &str) -> Option<Scope> {
        let mut out = Scope { write: WriteScope::Read, tools: ToolsetMask::DEFAULT };
        for part in s.split(' ').filter(|p| !p.is_empty()) {
            match part.split_once(':') {
                Some(("write", w)) => out.write = WriteScope::parse_wire(w)?,
                Some(("tools", t)) => {
                    let mut m = ToolsetMask(0);
                    for name in t.split(',') {
                        m.0 |= ToolsetMask::of(*Toolset::ALL.iter().find(|x| x.wire() == name)?).0;
                    }
                    out.tools = m;
                }
                None if part == "offline_access" => {}
                _ => return None,
            }
        }
        Some(out)
    }

    pub fn wire(self) -> String {
        format!("write:{} tools:{}", self.write.wire(), self.tools.wire())
    }

    /// A refresh or exchange may only narrow: no higher write scope and no toolset the grant lacks.
    pub fn within(self, grant: Scope) -> bool {
        self.write <= grant.write && self.tools.within(grant.tools)
    }
}

/// RFC 7636 S256: the challenge is the verifier's SHA-256, base64url without padding.
pub fn s256(verifier: &str) -> String {
    B64.encode(Sha256::digest(verifier.as_bytes()))
}

/// A challenge S256 can have produced: 43 base64url characters.
pub fn challenge_ok(c: &str) -> bool {
    c.len() == 43 && c.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// RFC 9728 protected resource metadata. Only the read scope is listed, because SDK clients request every listed
/// scope [measured, `tests/m0/results/m0-wp15.md`, L0-08].
pub fn resource_metadata(origin: &str) -> Value {
    json!({
        "resource": format!("{origin}/"), "authorization_servers": [origin], "bearer_methods_supported": ["header"],
        "scopes_supported": ["write:read"], "resource_name": "Graftyard MCP",
    })
}

/// RFC 8414 authorization server metadata: PKCE S256, Client ID Metadata Documents, and RFC 9207's `iss`.
pub fn server_metadata(origin: &str) -> Value {
    json!({
        "issuer": origin, "authorization_endpoint": format!("{origin}/authorize"),
        "token_endpoint": format!("{origin}/token"), "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code", "refresh_token"], "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["none"], "client_id_metadata_document_supported": true,
        "authorization_response_iss_parameter_supported": true,
        "scopes_supported": ["write:read", "write:write", "write:write_approve_merge"],
    })
}

/// A client's metadata document, the fields `graft-mcp` reads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CimdDoc {
    pub client_id: String,
    #[serde(default)]
    pub client_name: Option<String>,
    pub redirect_uris: Vec<String>,
}

/// What a CIMD fetch returned: its status, `Cache-Control`, and at most `CIMD_MAX_BYTES + 1` bytes of body, so a longer
/// one is refused without reading it all. The fetch itself never follows a redirect and stops at `CIMD_TIMEOUT_MS`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fetched {
    pub status: u16,
    pub cache_control: Option<String>,
    pub body: Vec<u8>,
}

/// Whether `id` may be fetched: an https URL with a path, no fragment or credentials, on no zone in `deny` (the
/// install's own zones, so a client id cannot make `graft-mcp` call the install).
pub fn cimd_url(id: &str, deny: &[String]) -> Result<url::Url, &'static str> {
    let u = url::Url::parse(id).map_err(|_| "client_id is not a URL")?;
    if u.scheme() != "https" || u.path() == "/" || u.fragment().is_some() || !u.username().is_empty() {
        return Err("client_id must be an https URL with a path");
    }
    let host = u.host_str().unwrap_or_default();
    if deny.iter().any(|d| !d.is_empty() && (host == d || host.ends_with(&format!(".{d}")))) {
        return Err("client_id is on a denied zone");
    }
    Ok(u)
}

/// A fetched document's checks: 200 (a redirect is refused, not followed), at most 5 KB, JSON whose `client_id` is
/// its own URL, and at least one redirect URI. Returns the document and how many seconds it may be cached.
pub fn cimd_doc(id: &str, f: &Fetched) -> Result<(CimdDoc, u64), &'static str> {
    if f.status != 200 {
        return Err("client metadata fetch did not answer 200");
    }
    if f.body.len() > CIMD_MAX_BYTES {
        return Err("client metadata over 5 KB");
    }
    let doc: CimdDoc = serde_json::from_slice(&f.body).map_err(|_| "client metadata is not JSON")?;
    if doc.client_id != id {
        return Err("client metadata client_id does not equal its URL");
    }
    if doc.redirect_uris.is_empty() || doc.redirect_uris.iter().any(|r| url::Url::parse(r).is_err()) {
        return Err("client metadata has no usable redirect_uris");
    }
    let cc = f.cache_control.as_deref().unwrap_or_default();
    let max_age = if cc.contains("no-store") || cc.contains("no-cache") {
        0
    } else {
        cc.split(',')
            .find_map(|d| d.trim().strip_prefix("max-age="))
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0)
            .min(CIMD_CACHE_MAX_S)
    };
    Ok((doc, max_age))
}

/// The redirect URIs' hosts, each once, and whether all of them are this computer.
pub fn redirect_hosts(doc: &CimdDoc) -> (Vec<String>, bool) {
    let mut hosts: Vec<String> = Vec::new();
    for r in &doc.redirect_uris {
        let h = url::Url::parse(r).ok().and_then(|u| u.host_str().map(str::to_owned)).unwrap_or_default();
        if !hosts.contains(&h) {
            hosts.push(h);
        }
    }
    let local = hosts.iter().all(|h| matches!(h.as_str(), "localhost" | "127.0.0.1" | "[::1]"));
    (hosts, local)
}

/// The repositories a consent names: 1 to `REPOS_MAX` `owner/name`s, all of one owner.
pub fn parse_repos(s: &str) -> Option<Vec<String>> {
    let ok = |p: &str| {
        !p.is_empty() && p.len() <= 100 && p.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    };
    let repos: Vec<String> = s.split(',').map(str::trim).filter(|r| !r.is_empty()).map(str::to_owned).collect();
    let owner = repos.first()?.split_once('/')?.0.to_owned();
    let fine = repos.len() <= REPOS_MAX
        && repos.iter().all(|r| r.split_once('/').is_some_and(|(o, n)| o == owner && ok(o) && ok(n)));
    fine.then_some(repos)
}
