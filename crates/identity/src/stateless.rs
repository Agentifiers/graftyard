//! Stateless installation tokens (`design/SHARED.md`, "Identity and tokens"; `design/DESIGN.md`, Shape, "Identity";
//! `build/BUILD-PLAN.md`, M3-WP4).
//!
//! A token is `ghs_`, the App id in decimal, `_`, then a compact JWS: `header.payload.signature`, each base64url with
//! no padding. The header names `alg: EdDSA` and the signing key's `kid`; the payload is the `InstallationClaims`.
//! This is GitHub's `ghs_APPID_JWT` shape (`research/agents.md`, finding 4.3; `research/wire-census.md` #30), so it
//! matches `ghs_[A-Za-z0-9\.\-_]{36,}` and its length varies with the claims. Nothing here assumes 40 characters.
//!
//! - **Minting** is `graft-ops`'s. Its signing key is wrapped by a Secrets Store key; `KeySource` is the host's unwrap,
//!   and `Minter` caches the unwrapped key for at most `SIGNING_KEY_CACHE`.
//! - **Verification** is the edge's: offline shape, then the `kid`'s `VerifyingKey` row on `TokenShard(0)`, the
//!   signature, the time window, and the row's `retired` time. Revocation by `jti` is read from `jti_shard` by the
//!   leader once per entry (`crate::credential`).
//! - **Rotation.** A new key is published at least one token lifetime before it signs, and the old key's `retired`
//!   is set past the last `exp` any isolate could still sign with it, so both verify during the overlap
//!   (`rotation_step`).
//! - **Narrowing** (`narrow`) is `POST /app/installations/{id}/access_tokens`: `repositories`, `repository_ids`, and
//!   `permissions`, never beyond the installation's or the App's.

use std::collections::BTreeMap;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use ed25519_dalek::{Signer as _, SigningKey, VerifyingKey};
use graftyard_types::{RepoId, Timestamp, Ulid};
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

use crate::agents::SessionId;
use crate::apps::{Access, AppId, InstallationId, PermissionSet};
use crate::tokens::{Ed25519Public, Jti, Kid, RepoScope, TokensRead};

/// "Installation tokens expire one hour from the time you create them" (`research/agents.md`, finding 4.2).
pub const INSTALLATION_TOKEN_TTL: Duration = Duration::from_secs(3600);
/// A job's `GITHUB_TOKEN` lives at most this long: "because the `GITHUB_TOKEN` is an installation access token, it
/// can only be refreshed for up to 24 hours" [doc: GHES 3.22, "GITHUB_TOKEN"]. Only the Actions App's tokens may live
/// past `INSTALLATION_TOKEN_TTL`, and `acquirejob` alone mints one (`JobToken`); the job-end `jti` deny is the real
/// bound (`decisions/DECISIONS.md`, "`gh-token`").
pub const JOB_TOKEN_MAX: Duration = Duration::from_secs(24 * 3600);

/// The install-owned Actions App, whose installation a job's `GITHUB_TOKEN` is and whose check runs `RunOwner` writes
/// (`design/DESIGN.md`, `ActorId`): id 2 of cell 0's block 0, which `Ids` never leases, beside the install account's
/// id 1 [inference: an install-wide App needs an id no account or App takes; block 0 is the only range nothing else
/// assigns]. It has no `AppRecord`: no slug, key, client, or webhook, so no App route, token request, or delivery
/// reaches it.
pub const ACTIONS_APP: AppId = AppId(graftyard_types::DbId(2));

/// The `GITHUB_TOKEN` scopes GHES 3.22 lists, by their App permission names: the workflow's `pull-requests` is the
/// token's `pull_requests` [doc: GHES 3.22, "Workflow syntax for GitHub Actions", `permissions`, and "Permissions
/// required for GitHub Apps"]. `read-all` and `write-all` set each of them.
pub const ACTIONS_SCOPES: [&str; 12] = [
    "actions",
    "checks",
    "contents",
    "deployments",
    "discussions",
    "issues",
    "packages",
    "pages",
    "pull_requests",
    "repository_projects",
    "security_events",
    "statuses",
];

/// The Actions App's installation on `owner`: the owner's id negated. Every id `Ids` assigns is positive (`DbId::pack`
/// of a cell, block, and offset), so no installation registered through `RegisterInstallation` takes one, and an
/// owner has exactly one.
pub fn actions_installation(owner: graftyard_types::AccountId) -> InstallationId {
    InstallationId(graftyard_types::DbId(-owner.0.0))
}

/// Whether an installation is the Actions App's, which every listing of installations leaves out, as GHES lists no
/// `github-actions` installation [inference: GHES 3.22's installation routes take an App JWT or a user's token and
/// name only Apps the caller registered or installed].
pub fn is_actions_installation(id: InstallationId) -> bool {
    id.0.0 < 0
}

/// The longest a token of `app` may live: `JOB_TOKEN_MAX` for the Actions App, `INSTALLATION_TOKEN_TTL` for any other.
pub fn max_lifetime(app: AppId) -> Duration {
    if app == ACTIONS_APP { JOB_TOKEN_MAX } else { INSTALLATION_TOKEN_TTL }
}
/// DESIGN's: at most this long per `graft-ops` isolate.
pub const SIGNING_KEY_CACHE: Duration = Duration::from_secs(3600);
/// How far a token's `iat` may be ahead of the verifier's clock [guess].
pub const CLOCK_SKEW: Duration = Duration::from_secs(60);
/// The most repositories one token may name, as GitHub's endpoint [unverified for GHES 3.22].
pub const TOKEN_REPOS_MAX: usize = 500;

const PREFIX: &str = "ghs_";

fn ms(d: Duration) -> u64 {
    d.as_millis() as u64
}

/// SHARED's `InstallationClaims`. Its serde form is what an issue or merge command under the token carries across a
/// binding (format 32.9). `task` is AM1-WP3's: set only when the token request's additive `graftyard_task` named a live
/// Graftyard-run task, and absent from the serde form, the JWT, and every encoding when it is `None`, so a token
/// without one keeps M3-WP4's bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallationClaims {
    pub app: AppId,
    pub installation: InstallationId,
    pub repos: RepoScope,
    pub permissions: PermissionSet,
    pub jti: Jti,
    /// Whole seconds, as a JWT carries them.
    pub iat: Timestamp,
    pub exp: Timestamp,
    pub kid: Kid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<SessionId>,
}

impl InstallationClaims {
    /// The access a claim grants on one repository, before the installation's current selection is intersected.
    pub fn grants(&self, repo: RepoId) -> bool {
        match &self.repos {
            RepoScope::All => true,
            RepoScope::Selected(r) => r.contains(&repo),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum StatelessError {
    #[error("not a ghs_ token")]
    Prefix,
    #[error("not ghs_APPID_ then a compact JWS")]
    Shape,
    #[error("the JWS header is not EdDSA with a kid")]
    Header,
    #[error("the claims do not parse")]
    Claims,
    #[error("the App id before the JWS is not the claims' App")]
    AppMismatch,
    #[error("no verifying key has this kid")]
    UnknownKid,
    #[error("the key that signed it is retired")]
    Retired,
    #[error("bad signature")]
    Signature,
    #[error("expired")]
    Expired,
    #[error("issued in the future")]
    NotYet,
    #[error("lives longer than an installation token")]
    Lifetime,
}

#[derive(Serialize, Deserialize)]
struct Header {
    alg: String,
    typ: String,
    kid: String,
}

/// The JWT payload. `repos` is absent for every repository the installation reaches.
#[derive(Serialize, Deserialize)]
struct Payload {
    iss: i64,
    ins: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    repos: Option<Vec<i64>>,
    perms: BTreeMap<String, String>,
    jti: String,
    iat: u64,
    exp: u64,
    /// The `task` claim's string form (`SessionId::text`), only when set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    task: Option<String>,
}

fn level(a: Access) -> &'static str {
    match a {
        Access::Read => "read",
        Access::Write => "write",
        Access::Admin => "admin",
    }
}

fn parse_level(s: &str) -> Option<Access> {
    match s {
        "read" => Some(Access::Read),
        "write" => Some(Access::Write),
        "admin" => Some(Access::Admin),
        _ => None,
    }
}

impl Payload {
    fn of(c: &InstallationClaims) -> Payload {
        Payload {
            iss: c.app.0.0,
            ins: c.installation.0.0,
            repos: match &c.repos {
                RepoScope::All => None,
                RepoScope::Selected(r) => Some(r.iter().map(|r| r.0.0).collect()),
            },
            perms: c.permissions.0.iter().map(|(k, a)| (k.to_string(), level(*a).to_owned())).collect(),
            jti: c.jti.0.to_string(),
            iat: c.iat.0 / 1000,
            exp: c.exp.0 / 1000,
            task: c.task.as_ref().map(SessionId::text),
        }
    }

    fn claims(self, kid: Kid) -> Option<InstallationClaims> {
        use graftyard_types::DbId;
        let task = match self.task {
            None => None,
            Some(t) => Some(SessionId::parse(&t, None)?),
        };
        let mut permissions = BTreeMap::new();
        for (k, v) in self.perms {
            permissions.insert(k.into_boxed_str(), parse_level(&v)?);
        }
        Some(InstallationClaims {
            app: AppId(DbId(self.iss)),
            installation: InstallationId(DbId(self.ins)),
            repos: match self.repos {
                None => RepoScope::All,
                Some(r) => RepoScope::Selected(r.into_iter().map(|r| RepoId(DbId(r))).collect()),
            },
            permissions: PermissionSet(permissions),
            jti: Jti(self.jti.parse::<Ulid>().ok()?),
            iat: Timestamp(self.iat.checked_mul(1000)?),
            exp: Timestamp(self.exp.checked_mul(1000)?),
            kid,
            task,
        })
    }
}

/// A `ghs_` token whose shape parsed, not yet verified: use `claims` only after `verify` succeeds.
#[derive(Clone, Debug)]
pub struct Unverified {
    claims: InstallationClaims,
    signed: String,
    sig: [u8; 64],
}

/// The edge's offline check: prefix, the App id, three base64url parts, an EdDSA header with a `kid`, and claims
/// that parse and name the same App. No store is read.
pub fn parse(text: &str) -> Result<Unverified, StatelessError> {
    let rest = text.strip_prefix(PREFIX).ok_or(StatelessError::Prefix)?;
    let (app, jws) = rest.split_once('_').ok_or(StatelessError::Shape)?;
    if app.is_empty() || !app.bytes().all(|b| b.is_ascii_digit()) {
        return Err(StatelessError::Shape);
    }
    let app: i64 = app.parse().map_err(|_| StatelessError::Shape)?;
    let mut parts = jws.split('.');
    let (Some(h), Some(p), Some(s), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
        return Err(StatelessError::Shape);
    };
    let header: Header =
        B64.decode(h).ok().and_then(|b| serde_json::from_slice(&b).ok()).ok_or(StatelessError::Header)?;
    if header.alg != "EdDSA" || header.typ != "JWT" {
        return Err(StatelessError::Header);
    }
    let kid = Kid(header.kid.parse().map_err(|_| StatelessError::Header)?);
    let payload: Payload =
        B64.decode(p).ok().and_then(|b| serde_json::from_slice(&b).ok()).ok_or(StatelessError::Claims)?;
    let claims = payload.claims(kid).ok_or(StatelessError::Claims)?;
    if claims.app.0.0 != app {
        return Err(StatelessError::AppMismatch);
    }
    let sig: [u8; 64] = B64.decode(s).ok().and_then(|b| b.try_into().ok()).ok_or(StatelessError::Shape)?;
    Ok(Unverified { claims, signed: format!("{h}.{p}"), sig })
}

impl Unverified {
    pub fn kid(&self) -> Kid {
        self.claims.kid
    }

    /// The claimed `jti`, before the signature is checked: only to start its revocation read early.
    pub fn jti(&self) -> Jti {
        self.claims.jti
    }

    /// The claims once the signature verifies under `key` and the token is inside its window at `now`.
    pub fn verify(self, key: &Ed25519Public, now: Timestamp) -> Result<InstallationClaims, StatelessError> {
        let vk = VerifyingKey::from_bytes(&key.0).map_err(|_| StatelessError::Signature)?;
        vk.verify_strict(self.signed.as_bytes(), &ed25519_dalek::Signature::from_bytes(&self.sig))
            .map_err(|_| StatelessError::Signature)?;
        let c = self.claims;
        if c.exp.0.saturating_sub(c.iat.0) > ms(max_lifetime(c.app)) {
            return Err(StatelessError::Lifetime);
        }
        if c.iat.0 > now.0 + ms(CLOCK_SKEW) {
            return Err(StatelessError::NotYet);
        }
        if c.exp <= now {
            return Err(StatelessError::Expired);
        }
        Ok(c)
    }
}

/// The edge's verification path: `text` checked offline, its `kid` looked up on `TokenShard(0)` (`keys`), and the
/// token refused once its key's `retired` time has passed. Revocation by `jti` is not read here: a read-class
/// request honors it within `READ_CACHE` from the `jti_shard` row, and every write's leader reads that row.
pub fn verify(text: &str, keys: &dyn TokensRead, now: Timestamp) -> Result<InstallationClaims, StatelessError> {
    let t = parse(text)?;
    let row = keys.verifying_key(t.kid()).ok_or(StatelessError::UnknownKid)?;
    if row.retired.is_some_and(|r| r <= now) {
        return Err(StatelessError::Retired);
    }
    t.verify(&row.key, now)
}

// ---- minting in `graft-ops` ----

/// The installation-token signing key, unwrapped. Never logged or serialized.
pub struct SigningSecret(pub [u8; 32]);

impl std::fmt::Debug for SigningSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SigningSecret(..)")
    }
}

impl SigningSecret {
    pub fn public(&self) -> Ed25519Public {
        Ed25519Public(SigningKey::from_bytes(&self.0).verifying_key().to_bytes())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum KeyError {
    /// The Secrets Store key did not unwrap the signing key, or it is unreachable: mint nothing (503).
    #[error("the signing key could not be unwrapped: {0}")]
    Unwrap(Box<str>),
}

/// Where `graft-ops` gets the current signing key: the wrapped bytes and their `kid`, unwrapped with the Secrets Store
/// key. The host implements it; `Minter` calls it at most once per `SIGNING_KEY_CACHE`.
pub trait KeySource {
    fn current(&self) -> Result<(Kid, SigningSecret), KeyError>;
}

/// What the token request asks for after `narrow`: the installation, its App, and the token's scope. The edge sends
/// it to `graft-ops` over the binding as JSON. `task` is set only after the edge checked the request's
/// `graftyard_task` against the task's row (`graftyard_git::task::token_task`), and is absent from the JSON otherwise.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Grant {
    pub app: AppId,
    pub installation: InstallationId,
    pub repos: RepoScope,
    pub permissions: PermissionSet,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<SessionId>,
    /// How long the token lives, in milliseconds, when not `INSTALLATION_TOKEN_TTL`: only `acquirejob` sets it, for a
    /// job's `GITHUB_TOKEN` (`job_token_life`). `mint` caps it at `max_lifetime(app)`, so another App's grant that names
    /// one still gets an hour. Absent from the JSON when `None`, so every other grant keeps its bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifetime_ms: Option<u64>,
    /// The token's `jti` when the caller chose it: `acquirejob` names the job's own (`graftyard_actions::token::job_jti`),
    /// so `RunOwner` can deny it when the job ends without storing it. Absent from the JSON when `None`, and then
    /// `mint` draws one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jti: Option<Jti>,
}

/// A job's `GITHUB_TOKEN` lifetime: the job's limit plus one lease, so a job that runs to its limit and whose runner
/// then vanishes still holds a live token until the lease fails the job and `RunOwner` denies it, capped at
/// `JOB_TOKEN_MAX` (`decisions/DECISIONS.md`, "`gh-token`").
pub fn job_token_life(job_limit: Duration, lease: Duration) -> Duration {
    (job_limit + lease).min(JOB_TOKEN_MAX)
}

/// One `graft-ops` isolate's minter: the unwrapped key and when it was fetched.
#[derive(Default)]
pub struct Minter {
    held: Option<(Kid, SigningKey, Timestamp)>,
    /// How many times the source was asked, for the cache's bound.
    pub fetches: u64,
}

impl Minter {
    /// Whether the next `mint` at `now` asks its `KeySource`: no key is held, or the one held is `SIGNING_KEY_CACHE`
    /// old. A host whose key read is asynchronous (Secrets Store) reads it only then.
    pub fn needs_key(&self, now: Timestamp) -> bool {
        !self.held.as_ref().is_some_and(|(_, _, at)| now.0.saturating_sub(at.0) < ms(SIGNING_KEY_CACHE))
    }

    /// Signs a token for `grant` at `now`. `rand` is the `jti`'s 80 random bits, which the host draws.
    pub fn mint(
        &mut self,
        src: &dyn KeySource,
        grant: &Grant,
        now: Timestamp,
        rand: [u8; 10],
    ) -> Result<(String, InstallationClaims), KeyError> {
        if self.needs_key(now) {
            self.held = None;
            self.fetches += 1;
            let (kid, secret) = src.current()?;
            self.held = Some((kid, SigningKey::from_bytes(&secret.0), now));
        }
        let (kid, key, _) = self.held.as_ref().expect("held");
        let iat = Timestamp(now.0 / 1000 * 1000);
        let life = grant.lifetime_ms.unwrap_or(ms(INSTALLATION_TOKEN_TTL)).min(ms(max_lifetime(grant.app)));
        let claims = InstallationClaims {
            app: grant.app,
            installation: grant.installation,
            repos: grant.repos.clone(),
            permissions: grant.permissions.clone(),
            jti: grant.jti.unwrap_or(Jti(Ulid::new(now, rand))),
            iat,
            exp: Timestamp(iat.0 + life / 1000 * 1000),
            kid: *kid,
            task: grant.task.clone(),
        };
        Ok((sign(key, &claims), claims))
    }
}

fn sign(key: &SigningKey, c: &InstallationClaims) -> String {
    let header = Header { alg: "EdDSA".into(), typ: "JWT".into(), kid: c.kid.0.to_string() };
    let h = B64.encode(serde_json::to_vec(&header).expect("header"));
    let p = B64.encode(serde_json::to_vec(&Payload::of(c)).expect("payload"));
    let signed = format!("{h}.{p}");
    let s = B64.encode(key.sign(signed.as_bytes()).to_bytes());
    format!("{PREFIX}{}_{signed}.{s}", c.app.0.0)
}

/// Signs `claims` with `secret` directly: for tests and the lifeboat, which hold no `Minter`.
pub fn sign_with(secret: &SigningSecret, claims: &InstallationClaims) -> String {
    sign(&SigningKey::from_bytes(&secret.0), claims)
}

// ---- rotation ----

/// One signing key's life, as `graft-ops` records it: when its `VerifyingKey` row committed, and when the key source
/// switched to the next key (`None` while it is the source's current key).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyEpoch {
    pub kid: Kid,
    pub published: Timestamp,
    pub switched: Option<Timestamp>,
}

/// What a rotation does next: switch the key source to the new key once its row has been published for one
/// installation-token lifetime, so every edge has had time to see it; then retire the old key once no token it signed
/// can still be live. An isolate may hold the old key for `SIGNING_KEY_CACHE` after the switch, and its last token is
/// a job's `GITHUB_TOKEN`, which lives up to `JOB_TOKEN_MAX`, so the old key retires at the switch plus both
/// (`decisions/DECISIONS.md`, "`gh-token`").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RotationStep {
    /// Keep signing with the old key.
    Wait,
    /// Switch the key source to the new key now.
    SignWithNew,
    /// Commit `PutVerifyingKey { kid, retired: Some(at) }` for the old key.
    Retire { kid: Kid, at: Timestamp },
}

/// The next step from `old` to `new` at `now`.
pub fn rotation_step(old: &KeyEpoch, new: &KeyEpoch, now: Timestamp) -> RotationStep {
    match old.switched {
        None if now.0 >= new.published.0 + ms(INSTALLATION_TOKEN_TTL) => RotationStep::SignWithNew,
        None => RotationStep::Wait,
        Some(at) => RotationStep::Retire {
            kid: old.kid,
            at: Timestamp(at.0 + ms(SIGNING_KEY_CACHE) + ms(JOB_TOKEN_MAX) + ms(CLOCK_SKEW)),
        },
    }
}

// ---- narrowing: `POST /app/installations/{id}/access_tokens` ----

/// The request body's three narrowing fields. `repositories` are names under the installation's account.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AccessTokenRequest {
    pub repositories: Vec<Box<str>>,
    pub repository_ids: Vec<RepoId>,
    pub permissions: Option<PermissionSet>,
}

/// Why `narrow` refused: each is GitHub's 422.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum NarrowError {
    /// "The permissions requested are not granted to this installation."
    #[error("permission {0:?} is not granted to this installation")]
    NotGranted(Box<str>),
    #[error("repository {0:?} is not accessible to this installation")]
    Repository(Box<str>),
    #[error("over {TOKEN_REPOS_MAX} repositories")]
    TooMany,
}

/// The token's scope: the installation's selection and permissions, narrowed by the request. A permission the App or
/// the installation lacks, or above their level, is refused; so is a repository outside the selection. `resolve`
/// maps a name to the id the installation's account holds it under.
pub fn narrow(
    selection: &RepoScope,
    installation: &PermissionSet,
    app: &PermissionSet,
    req: &AccessTokenRequest,
    resolve: &dyn Fn(&str) -> Option<RepoId>,
) -> Result<(RepoScope, PermissionSet), NarrowError> {
    let permissions = match &req.permissions {
        None => installation.clone(),
        Some(p) => {
            for (k, a) in &p.0 {
                let ok = |cap: &PermissionSet| cap.0.get(k).is_some_and(|c| a <= c);
                if !ok(app) || !ok(installation) {
                    return Err(NarrowError::NotGranted(k.clone()));
                }
            }
            p.clone()
        }
    };
    if req.repositories.len() + req.repository_ids.len() > TOKEN_REPOS_MAX {
        return Err(NarrowError::TooMany);
    }
    let inside = |r: RepoId| match selection {
        RepoScope::All => true,
        RepoScope::Selected(s) => s.contains(&r),
    };
    let mut ids: SmallVec<[RepoId; 4]> = SmallVec::new();
    for n in &req.repositories {
        match resolve(n) {
            Some(r) if inside(r) => ids.push(r),
            _ => return Err(NarrowError::Repository(n.clone())),
        }
    }
    for r in &req.repository_ids {
        if !inside(*r) {
            return Err(NarrowError::Repository(r.0.0.to_string().into()));
        }
        ids.push(*r);
    }
    ids.sort();
    ids.dedup();
    let repos = if ids.is_empty() { selection.clone() } else { RepoScope::Selected(ids) };
    Ok((repos, permissions))
}

// ---- the edge's two routes ----

/// Why `POST /app/installations/{id}/access_tokens` refused, before anything was minted.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TokenRequestError {
    /// 401: the App JWT did not verify.
    #[error("{0}")]
    Jwt(crate::appauth::AppJwtError),
    /// 404: no live installation with this id belongs to the JWT's App.
    #[error("no such installation")]
    NoInstallation,
    /// 422.
    #[error("{0}")]
    Narrow(NarrowError),
}

/// The edge's half of `POST /app/installations/{id}/access_tokens`, before it asks `graft-ops` to mint. `app` is the
/// App's record from its owner's `Authz` fold, reached through `NameKey::App`; `installation` the installation's
/// owner's `Authz` fold, reached through `NameKey::Installation`. Both were read fresh. Returns the grant to sign.
pub fn access_token_grant(
    jwt: &str,
    app: &crate::apps::AppRecord,
    id: InstallationId,
    installation: &dyn crate::authz::AuthzRead,
    req: &AccessTokenRequest,
    resolve: &dyn Fn(&str) -> Option<RepoId>,
    now: Timestamp,
) -> Result<Grant, TokenRequestError> {
    crate::appauth::verify_app_jwt(jwt, app, now).map_err(TokenRequestError::Jwt)?;
    let row = installation
        .installation(id)
        .and_then(|s| s.row.as_ref())
        .filter(|r| r.app == app.id)
        .ok_or(TokenRequestError::NoInstallation)?;
    let (repos, permissions) =
        narrow(&row.repos, &row.permissions, &app.permissions, req, resolve).map_err(TokenRequestError::Narrow)?;
    Ok(Grant { app: app.id, installation: id, repos, permissions, task: None, lifetime_ms: None, jti: None })
}

/// `DELETE /installation/token`: the command the edge commits on `jti_shard` under the token itself. The leader
/// accepts it only from that token, or from `graft-ops`; it answers 204 for a token already revoked or expired.
pub fn revoke_command(claims: &InstallationClaims) -> crate::tokens::TokensCmd {
    crate::tokens::TokensCmd {
        cred: crate::credential::Principal::installation(claims).credential_shard(),
        op: crate::tokens::TokensOp::RevokeJti { jti: claims.jti, exp: claims.exp },
    }
}

#[cfg(test)]
mod tests {
    use graftyard_types::DbId;

    use super::*;
    use crate::tokens::{TokensEvent, TokensFold};
    use graftyard_raft::Fold as _;
    use graftyard_types::TokenShard;

    struct Src(Kid, [u8; 32]);
    impl KeySource for Src {
        fn current(&self) -> Result<(Kid, SigningSecret), KeyError> {
            Ok((self.0, SigningSecret(self.1)))
        }
    }

    fn perms(p: &[(&str, Access)]) -> PermissionSet {
        PermissionSet(p.iter().map(|(k, a)| (Box::from(*k), *a)).collect())
    }

    fn grant(repos: RepoScope) -> Grant {
        Grant {
            app: AppId(DbId(1_234_567)),
            installation: InstallationId(DbId(42)),
            repos,
            permissions: perms(&[("contents", Access::Write), ("metadata", Access::Read)]),
            task: None,
            lifetime_ms: None,
            jti: None,
        }
    }

    fn keys(kid: Kid, secret: [u8; 32], retired: Option<Timestamp>) -> TokensFold {
        let mut f = TokensFold::new(TokenShard(0));
        f.apply(&TokensEvent::VerifyingKey { kid, key: SigningSecret(secret).public(), retired });
        f
    }

    #[test]
    fn a_token_is_ghs_appid_then_a_jws_of_varying_length_and_verifies() {
        let now = Timestamp(1_800_000_000_000);
        let src = Src(Kid(3), [5; 32]);
        let mut m = Minter::default();
        let f = keys(Kid(3), [5; 32], None);
        let mut lens = Vec::new();
        for n in [0usize, 1, 10, 100] {
            let repos = if n == 0 {
                RepoScope::All
            } else {
                RepoScope::Selected((0..n as i64).map(|i| RepoId(DbId(1_000 + i))).collect())
            };
            let (t, c) = m.mint(&src, &grant(repos), now, [n as u8; 10]).unwrap();
            assert!(t.starts_with("ghs_1234567_"), "{t}");
            assert!(t.len() > 40);
            assert!(t[4..].bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)), "Renovate's regex");
            assert_eq!(t.matches('.').count(), 2);
            assert_eq!(verify(&t, &f, now), Ok(c));
            lens.push(t.len());
        }
        assert!(lens.windows(2).all(|w| w[0] < w[1]), "length varies with the claims: {lens:?}");
        assert_eq!(m.fetches, 1);
    }

    /// AM1-WP3: a grant's `task` becomes the token's `task` claim and verifies back; a token without one carries no
    /// `task` member at all, so its payload is M3-WP4's.
    #[test]
    fn a_task_claim_round_trips_and_its_absence_keeps_the_payload() {
        let now = Timestamp(1_800_000_000_000);
        let src = Src(Kid(3), [5; 32]);
        let f = keys(Kid(3), [5; 32], None);
        let task = SessionId::parse("0193a1b2-c3d4-8e5f-8a9b-0c1d2e3f4a5b", None).unwrap();
        let mut g = grant(RepoScope::Selected(smallvec::smallvec![RepoId(DbId(7))]));
        let (plain, pc) = Minter::default().mint(&src, &g, now, [1; 10]).unwrap();
        let payload = |t: &str| String::from_utf8(B64.decode(t.split('.').nth(1).unwrap()).unwrap()).unwrap();
        assert!(!payload(&plain).contains("task"), "{}", payload(&plain));
        assert_eq!(verify(&plain, &f, now).unwrap().task, None);
        assert!(!serde_json::to_string(&pc).unwrap().contains("task"), "the claims' serde form keeps M3-WP4's");
        g.task = Some(task.clone());
        let (t, c) = Minter::default().mint(&src, &g, now, [1; 10]).unwrap();
        assert!(payload(&t).contains(&format!("\"task\":\"{}\"", task.text())));
        assert_eq!(verify(&t, &f, now).unwrap().task, Some(task));
        assert_eq!(verify(&t, &f, now), Ok(c));
    }

    #[test]
    fn a_tampered_expired_or_unknown_key_token_is_refused() {
        let now = Timestamp(1_800_000_000_000);
        let mut m = Minter::default();
        let (t, _) = m.mint(&Src(Kid(3), [5; 32]), &grant(RepoScope::All), now, [1; 10]).unwrap();
        let f = keys(Kid(3), [5; 32], None);
        // Another payload under the same signature: wider repositories.
        let (_, jws) = t[4..].split_once('_').unwrap();
        let mut parts: Vec<&str> = jws.split('.').collect();
        let other = m.mint(
            &Src(Kid(3), [5; 32]),
            &grant(RepoScope::Selected(SmallVec::from_slice(&[RepoId(DbId(9))]))),
            now,
            [2; 10],
        );
        let other = other.unwrap().0;
        let (_, ojws) = other[4..].split_once('_').unwrap();
        let op: Vec<&str> = ojws.split('.').collect();
        parts[1] = op[1];
        let forged = format!("ghs_1234567_{}", parts.join("."));
        assert_eq!(verify(&forged, &f, now), Err(StatelessError::Signature));
        assert_eq!(verify(&t.replacen("ghs_1234567_", "ghs_7654321_", 1), &f, now), Err(StatelessError::AppMismatch));
        assert_eq!(verify(&t, &keys(Kid(4), [5; 32], None), now), Err(StatelessError::UnknownKid));
        assert_eq!(verify(&t, &keys(Kid(3), [6; 32], None), now), Err(StatelessError::Signature));
        let late = Timestamp(now.0 + ms(INSTALLATION_TOKEN_TTL));
        assert_eq!(verify(&t, &f, late), Err(StatelessError::Expired));
        assert_eq!(verify(&t, &f, Timestamp(now.0 - ms(CLOCK_SKEW) - 1_000)), Err(StatelessError::NotYet));
        for bad in ["ghp_x", "ghs_", "ghs_12a_x.y.z", "ghs_1_x.y", "ghs_1_x.y.z.w"] {
            assert!(verify(bad, &f, now).is_err(), "{bad}");
        }
    }

    /// A job's `GITHUB_TOKEN` lives the job limit plus a lease, up to `JOB_TOKEN_MAX`, and only the Actions App's
    /// tokens may: another App's grant naming a longer life is minted for an hour, and a token claiming more than its
    /// App's maximum is refused even under a good signature.
    #[test]
    fn only_the_actions_apps_tokens_outlive_an_hour_and_none_outlives_a_day() {
        let now = Timestamp(1_800_000_000_000);
        let (src, f) = (Src(Kid(3), [5; 32]), keys(Kid(3), [5; 32], None));
        let hour = ms(INSTALLATION_TOKEN_TTL);
        let six = job_token_life(Duration::from_secs(6 * 3600), Duration::from_secs(600));
        assert_eq!(ms(six), 6 * hour + 600_000);
        assert_eq!(job_token_life(Duration::from_secs(5 * 86_400), Duration::from_secs(600)), JOB_TOKEN_MAX);
        let named = Jti(Ulid::new(Timestamp(0), [9; 10]));
        let job = Grant {
            app: ACTIONS_APP,
            installation: actions_installation(graftyard_types::AccountId(DbId(77))),
            lifetime_ms: Some(ms(six)),
            jti: Some(named),
            ..grant(RepoScope::All)
        };
        let (t, c) = Minter::default().mint(&src, &job, now, [1; 10]).unwrap();
        assert_eq!(c.exp.0 - c.iat.0, ms(six));
        assert_eq!(c.jti, named, "the job's own jti, which RunOwner denies");
        assert_eq!(verify(&t, &f, Timestamp(now.0 + 6 * hour)), Ok(c), "alive past an hour");
        // Another App asks for the same life and gets an hour.
        let other = Grant { lifetime_ms: Some(ms(six)), ..grant(RepoScope::All) };
        let (t, c) = Minter::default().mint(&src, &other, now, [2; 10]).unwrap();
        assert_eq!(c.exp.0 - c.iat.0, hour);
        assert_eq!(c.jti, Jti(Ulid::new(now, [2; 10])), "a grant naming no jti gets a drawn one");
        assert_eq!(verify(&t, &f, Timestamp(now.0 + hour)), Err(StatelessError::Expired));
        // A day and a second asked of the Actions App is minted for a day.
        let long = Grant { lifetime_ms: Some(ms(JOB_TOKEN_MAX) + 1_000), ..job.clone() };
        let (_, c) = Minter::default().mint(&src, &long, now, [3; 10]).unwrap();
        assert_eq!(c.exp.0 - c.iat.0, ms(JOB_TOKEN_MAX));
        // Signed claims over each App's maximum: refused at verify.
        let iat = Timestamp(now.0 / 1000 * 1000);
        let over = |app, life| {
            let claims = InstallationClaims {
                app,
                installation: InstallationId(DbId(42)),
                repos: RepoScope::All,
                permissions: PermissionSet::default(),
                jti: Jti(Ulid::new(now, [4; 10])),
                iat,
                exp: Timestamp(iat.0 + life),
                kid: Kid(3),
                task: None,
            };
            verify(&sign_with(&SigningSecret([5; 32]), &claims), &f, now)
        };
        assert_eq!(over(AppId(DbId(1_234_567)), hour + 1_000), Err(StatelessError::Lifetime));
        assert!(over(ACTIONS_APP, hour + 1_000).is_ok());
        assert_eq!(over(ACTIONS_APP, ms(JOB_TOKEN_MAX) + 1_000), Err(StatelessError::Lifetime));
        // The Actions installation of an owner is its own and can never be a registered one.
        let o = graftyard_types::AccountId(DbId::pack(3, 7, 1).unwrap());
        assert!(is_actions_installation(actions_installation(o)));
        assert_ne!(actions_installation(o), actions_installation(graftyard_types::AccountId(DbId(o.0.0 + 1))));
        assert!(!is_actions_installation(InstallationId(DbId::pack(255, DbId::MAX_BLOCK, u16::MAX).unwrap())));
    }

    #[test]
    fn the_signing_key_is_cached_for_at_most_signing_key_cache() {
        let src = Src(Kid(1), [8; 32]);
        let mut m = Minter::default();
        let t0 = Timestamp(1_000_000_000);
        for s in 0..60u64 {
            m.mint(&src, &grant(RepoScope::All), Timestamp(t0.0 + s * 60_000), [0; 10]).unwrap();
        }
        m.mint(&src, &grant(RepoScope::All), Timestamp(t0.0 + ms(SIGNING_KEY_CACHE) - 1), [0; 10]).unwrap();
        assert_eq!(m.fetches, 1, "one fetch inside the hour");
        m.mint(&src, &grant(RepoScope::All), Timestamp(t0.0 + ms(SIGNING_KEY_CACHE)), [0; 10]).unwrap();
        assert_eq!(m.fetches, 2, "refetched at the hour");
    }

    #[test]
    fn narrowing_never_exceeds_the_installation_or_the_app() {
        let app = perms(&[("contents", Access::Write), ("issues", Access::Write), ("metadata", Access::Read)]);
        let inst = perms(&[("contents", Access::Write), ("metadata", Access::Read)]);
        let sel = RepoScope::Selected(SmallVec::from_slice(&[RepoId(DbId(1)), RepoId(DbId(2))]));
        let resolve = |n: &str| match n {
            "a" => Some(RepoId(DbId(1))),
            "b" => Some(RepoId(DbId(2))),
            "c" => Some(RepoId(DbId(3))),
            _ => None,
        };
        let req = |names: &[&str], ids: &[i64], p: Option<PermissionSet>| AccessTokenRequest {
            repositories: names.iter().map(|n| Box::from(*n)).collect(),
            repository_ids: ids.iter().map(|i| RepoId(DbId(*i))).collect(),
            permissions: p,
        };
        assert_eq!(narrow(&sel, &inst, &app, &req(&[], &[], None), &resolve), Ok((sel.clone(), inst.clone())));
        let (r, p) =
            narrow(&sel, &inst, &app, &req(&["a"], &[], Some(perms(&[("contents", Access::Read)]))), &resolve).unwrap();
        assert_eq!(
            (r, p),
            (RepoScope::Selected(SmallVec::from_slice(&[RepoId(DbId(1))])), perms(&[("contents", Access::Read)]))
        );
        let (r, _) = narrow(&sel, &inst, &app, &req(&["a"], &[2, 1], None), &resolve).unwrap();
        assert_eq!(r, RepoScope::Selected(SmallVec::from_slice(&[RepoId(DbId(1)), RepoId(DbId(2))])));
        let e = |n, i, p| narrow(&sel, &inst, &app, &req(n, i, p), &resolve).unwrap_err();
        assert_eq!(e(&["c"], &[], None), NarrowError::Repository("c".into()));
        assert_eq!(e(&["zz"], &[], None), NarrowError::Repository("zz".into()));
        assert_eq!(e(&[], &[3], None), NarrowError::Repository("3".into()));
        // `issues` the App has and the installation lacks; `admin` the App lacks; `pages` nobody has.
        assert_eq!(e(&[], &[], Some(perms(&[("issues", Access::Read)]))), NarrowError::NotGranted("issues".into()));
        assert_eq!(
            e(&[], &[], Some(perms(&[("contents", Access::Admin)]))),
            NarrowError::NotGranted("contents".into())
        );
        assert_eq!(e(&[], &[], Some(perms(&[("pages", Access::Read)]))), NarrowError::NotGranted("pages".into()));
        // The App lowered its permissions after the install: the installation's older, wider row does not grant.
        let lowered = perms(&[("contents", Access::Read), ("metadata", Access::Read)]);
        let wide = req(&[], &[], Some(perms(&[("contents", Access::Write)])));
        assert_eq!(narrow(&sel, &inst, &lowered, &wide, &resolve), Err(NarrowError::NotGranted("contents".into())));
    }

    #[test]
    fn rotation_signs_with_the_new_key_after_one_lifetime_and_retires_the_old_after_its_last_exp() {
        let h = ms(INSTALLATION_TOKEN_TTL);
        let old = KeyEpoch { kid: Kid(1), published: Timestamp(0), switched: None };
        let new = KeyEpoch { kid: Kid(2), published: Timestamp(10 * h), switched: None };
        assert_eq!(rotation_step(&old, &new, Timestamp(10 * h + h - 1)), RotationStep::Wait);
        assert_eq!(rotation_step(&old, &new, Timestamp(11 * h)), RotationStep::SignWithNew);
        let old = KeyEpoch { switched: Some(Timestamp(11 * h)), ..old };
        // A cached isolate may sign with kid 1 until 12 h, and a job's token it signs then lives until 36 h.
        assert_eq!(
            rotation_step(&old, &new, Timestamp(11 * h)),
            RotationStep::Retire { kid: Kid(1), at: Timestamp(36 * h + ms(CLOCK_SKEW)) }
        );
    }
}
