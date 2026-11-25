//! Actions OIDC (`design/SHARED.md`, "Actions", OIDC; `decisions/DECISIONS.md`, "`actions-oidc`"): the job's ID
//! token, its issuer's discovery document and JWKS, and who may mint one. Pure: the hosts read the rows and the key.
//!
//! - **The runner's wire.** The job message's `SystemVssConnection` carries `GenerateIdTokenUrl` only for a job granted
//!   `id-token: write` (`id_token_granted`), and the runner then exports it as `ACTIONS_ID_TOKEN_REQUEST_URL`, with the
//!   job token as `ACTIONS_ID_TOKEN_REQUEST_TOKEN` (actions/runner aea9aaa, `ScriptHandler.cs:315-318`). The toolkit's
//!   `oidc-utils.ts` appends `&audience=...`, sends a `GET` with that bearer, and reads `{"value"}`.
//! - **Who may mint** (`may_mint`): the job's current attempt, `InProgress` (the runner waits for it: "Server won't
//!   issue ID_TOKEN for non-inprogress job", `JobRunner.cs:214`), granted `id-token: write`, and not a fork's pull
//!   request.
//! - **The token** (`claims`): GHES 3.22's claim set [doc: GHES 3.22, "OpenID Connect reference", "OIDC token
//!   claims"], with the legacy `sub` (`subject`) and a 5-minute life, RS256 under `ActionsOidcKey`'s active slot with
//!   `kid` the key's RFC 7638 thumbprint. AWS STS takes only RSA or ECDSA keys [doc: AWS, `AssumeRoleWithWebIdentity`],
//!   so the install's Ed25519 keys cannot sign it.
//! - **The JWKS** (`published`) lists both slots' public keys, so the key a token was signed under stays published
//!   until the next rotation overwrites its slot (SHARED: "publishes every current and previous key epoch").

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD as B64S, URL_SAFE_NO_PAD as B64};
use graftyard_identity::{Access, PermissionSet, Visibility};
use graftyard_types::{AccountId, RepoId};
use rsa::pkcs8::DecodePrivateKey as _;
use rsa::traits::PublicKeyParts as _;
use rsa::{RsaPrivateKey, RsaPublicKey};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use crate::job::JobState;
use crate::shard::{JobRow, WorkflowRun};

/// An `ActionsOidcKey` slot's parsed key (`parse_key`), so a host names it without its own `rsa` dependency.
pub type OidcKey = RsaPrivateKey;

/// An ID token's life, `exp - iat` [doc: GitHub's example token in "OpenID Connect", `iat` 1632493567 and `exp`
/// 1632493867, 300 s apart].
pub const ID_TOKEN_TTL_S: u64 = 300;
/// How far `nbf` sits before `iat`, so a relying party whose clock runs behind the edge's still takes a fresh token
/// (F129). GitHub's tokens set `nbf` 600 s before `iat` [doc: GitHub's example token in "OpenID Connect", `nbf`
/// 1632492967 and `iat` 1632493567; the same offset in decoded github.com tokens, such as `nbf` 1700879858 and `iat`
/// 1700880458 (https://www.kenmuse.com/blog/understanding-oidc-and-identity-federation)]. It widens only the start of
/// the window: `exp` stays `iat + ID_TOKEN_TTL_S`, so a token is never usable longer after its mint.
pub const ID_TOKEN_NBF_SKEW_S: u64 = 600;
/// The declared-permissions key that records `id-token: write` (`crate::token::parse`). No `GITHUB_TOKEN` carries it
/// (`crate::token::job_permissions`).
pub const ID_TOKEN_SCOPE: &str = "id_token";
/// The issuer's path under the install (SHARED: `https://HOST/_services/token`).
pub const ISSUER_PATH: &str = "/_services/token";
/// The smallest key `parse_key` takes [inference: RSA-2048 is the plan's key; AWS STS takes RSA keys].
const KEY_BITS_MIN: usize = 2048;

/// The issuer for the install at `server` (`https://host`).
pub fn issuer(server: &str) -> String {
    format!("{}{ISSUER_PATH}", server.trim_end_matches('/'))
}

/// Whether the job's declared permissions grant an ID token: `id-token: write`, and not for a fork's pull request,
/// whose writes GHES drops [doc: GHES 3.22, "Workflow syntax", "How permissions are calculated for a workflow job"].
pub fn id_token_granted(declared: Option<&PermissionSet>, fork: bool) -> bool {
    !fork && declared.and_then(|p| p.0.get(ID_TOKEN_SCOPE)) == Some(&Access::Write)
}

/// Why an ID token is not minted; the run service answers 403 for each.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Refused {
    #[error("the job is not in progress")]
    NotRunning,
    #[error("the token names another attempt of the job")]
    Stale,
    #[error("the job is not granted id-token: write")]
    NoGrant,
    #[error("a fork's pull request gets no ID token")]
    Fork,
}

/// `may_mint` for job `row` of `run`, asked with a job token for `attempt`, with the job's declared permissions.
pub fn may_mint(
    row: &JobRow,
    run: &WorkflowRun,
    attempt: u16,
    declared: Option<&PermissionSet>,
) -> Result<(), Refused> {
    if run.attempt != attempt {
        return Err(Refused::Stale);
    }
    if !matches!(row.state, JobState::InProgress { .. }) {
        return Err(Refused::NotRunning);
    }
    if run.fork {
        return Err(Refused::Fork);
    }
    if !id_token_granted(declared, false) {
        return Err(Refused::NoGrant);
    }
    Ok(())
}

/// GHES's legacy `sub`: `repo:O/R:environment:E` for a job with an environment, else `repo:O/R:pull_request` for a
/// pull request event, else `repo:O/R:ref:REF`, every `:` in a value written `%3A` [doc: as the module docs,
/// "Example subject claims"].
pub fn subject(repository: &str, environment: Option<&str>, event: &str, r#ref: &str) -> String {
    let esc = |s: &str| s.replace(':', "%3A");
    let context = match environment {
        Some(e) => format!("environment:{}", esc(e)),
        None if matches!(event, "pull_request" | "pull_request_target") => "pull_request".into(),
        None => format!("ref:{}", esc(r#ref)),
    };
    format!("repo:{}:{context}", esc(repository))
}

/// What the token says about its job, read by the hosts: the run and job from the shard, the names, visibility and
/// enterprise from the owner's `Authz` and the repository's settings, and the runner's record.
pub struct Facts<'a> {
    /// `https://host`, the install.
    pub server: &'a str,
    /// `o/r`.
    pub repository: &'a str,
    pub repository_id: RepoId,
    pub owner_id: AccountId,
    pub visibility: Visibility,
    /// The enterprise's login and id, when the owner is an org in one.
    pub enterprise: Option<(&'a str, AccountId)>,
    pub run: &'a WorkflowRun,
    pub job: &'a JobRow,
    /// The job's runner is one of `graft-runners`' Sandboxes: `runner_environment` is `github-hosted`, as GHES says
    /// of platform runners (`decisions/DECISIONS.md`, "`actions-oidc`").
    pub hosted: bool,
    /// A `pull_request` run's base branch as the shard recorded it (`ActionsEvent::RunBase`, `refs/heads/main`), which
    /// `base_ref` carries as GitHub does, without `refs/heads/` (F121).
    pub base_ref: Option<&'a str>,
}

/// The token's claims at `now_s` with id `jti`, for `audience` or, without one, the owner's URL, GHES's default
/// ("the URL of the repository owner"). Every value GHES writes as a string is a string. The claims GHES has and this
/// leaves empty or out are `decisions/OPEN-QUESTIONS.md` rows: `actor` and `actor_id`, `head_ref`, `base_ref` for a
/// run with no recorded base, `run_number`, and a reusable workflow's `job_workflow_ref`.
pub fn claims(f: &Facts<'_>, audience: Option<&str>, now_s: u64, jti: &str) -> Value {
    let run = f.run;
    let owner = f.repository.split('/').next().unwrap_or("");
    let server = f.server.trim_end_matches('/');
    let aud = audience.filter(|a| !a.is_empty()).map_or_else(|| format!("{server}/{owner}"), String::from);
    let sha: String = run.head_sha.iter().map(|b| format!("{b:02x}")).collect();
    let r#ref = &*run.head_ref;
    let ref_type = if r#ref.starts_with("refs/tags/") { "tag" } else { "branch" };
    let environment = f.job.spec.environment.as_deref();
    let workflow_ref = format!("{}/{}@{}", f.repository, run.workflow, r#ref);
    let visibility = match f.visibility {
        Visibility::Public => "public",
        Visibility::Private => "private",
        Visibility::Internal => "internal",
    };
    let mut c = json!({
        "iss": issuer(server),
        "aud": aud,
        "sub": subject(f.repository, environment, &run.event, r#ref),
        "jti": jti,
        "iat": now_s,
        "nbf": now_s.saturating_sub(ID_TOKEN_NBF_SKEW_S),
        "exp": now_s + ID_TOKEN_TTL_S,
        "ref": r#ref,
        "ref_type": ref_type,
        "sha": sha,
        "repository": f.repository,
        "repository_id": f.repository_id.0.0.to_string(),
        "repository_owner": owner,
        "repository_owner_id": f.owner_id.0.0.to_string(),
        "repository_visibility": visibility,
        "run_id": run.id.0.0.to_string(),
        "run_attempt": run.attempt.to_string(),
        "event_name": &*run.event,
        "workflow": &*run.workflow,
        "workflow_ref": workflow_ref,
        "workflow_sha": sha,
        // An ordinary job's own workflow; a reusable workflow's is not built (`crate::workflow`).
        "job_workflow_ref": workflow_ref,
        "job_workflow_sha": sha,
        "runner_environment": if f.hosted { "github-hosted" } else { "self-hosted" },
        "head_ref": "",
        "base_ref": f.base_ref.map_or("", |b| b.strip_prefix("refs/heads/").unwrap_or(b)),
    });
    if let Some(e) = environment {
        c["environment"] = json!(e);
    }
    if let Some((login, id)) = f.enterprise {
        c["enterprise"] = json!(login);
        c["enterprise_id"] = json!(id.0.0.to_string());
    }
    c
}

/// The ID token for `f`'s job, asked with a job token for `attempt`, if `may_mint` allows it: `claims` signed under
/// `key`, the active slot's. Both hosts call it, the site's run service and `examples/runsvc_host.rs`.
#[allow(clippy::too_many_arguments)]
pub fn mint(
    f: &Facts<'_>,
    attempt: u16,
    declared: Option<&PermissionSet>,
    audience: Option<&str>,
    now_s: u64,
    jti: &str,
    key: &RsaPrivateKey,
) -> Result<String, Refused> {
    may_mint(f.job, f.run, attempt, declared)?;
    Ok(sign(key, &claims(f, audience, now_s, jti)))
}

/// Why a slot's text is not an `ActionsOidcKey`. Never holds the key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum KeyError {
    #[error("not a PEM PKCS#8 private key")]
    Pem,
    #[error("not an RSA private key")]
    Rsa,
    #[error("smaller than RSA-2048")]
    Small,
}

/// An `ActionsOidcKey` slot's value: an RSA private key of at least 2,048 bits as PEM PKCS#8 (`BEGIN PRIVATE KEY`),
/// as `openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048` writes it.
pub fn parse_key(text: &str) -> Result<RsaPrivateKey, KeyError> {
    let t = text.trim();
    let body = t
        .strip_prefix("-----BEGIN PRIVATE KEY-----")
        .and_then(|b| b.strip_suffix("-----END PRIVATE KEY-----"))
        .ok_or(KeyError::Pem)?;
    let b64: String = body.chars().filter(|c| !c.is_whitespace()).collect();
    let der = B64S.decode(b64).map_err(|_| KeyError::Pem)?;
    let k = RsaPrivateKey::from_pkcs8_der(&der).map_err(|_| KeyError::Rsa)?;
    if k.n().bits() < KEY_BITS_MIN {
        return Err(KeyError::Small);
    }
    Ok(k)
}

fn b64u(n: &rsa::BigUint) -> String {
    B64.encode(n.to_bytes_be())
}

/// The key's RFC 7638 JWK thumbprint: SHA-256 of `{"e","kty","n"}` in that order with no spaces, base64url.
pub fn kid(k: &RsaPublicKey) -> String {
    let canon = format!(r#"{{"e":"{}","kty":"RSA","n":"{}"}}"#, b64u(k.e()), b64u(k.n()));
    B64.encode(Sha256::digest(canon.as_bytes()))
}

/// The key's JWK, as the JWKS lists it.
pub fn jwk(k: &RsaPublicKey) -> Value {
    json!({ "kty": "RSA", "alg": "RS256", "use": "sig", "kid": kid(k), "n": b64u(k.n()), "e": b64u(k.e()) })
}

/// What a site publishes from its two slots' texts (slot 0 is A): `{"keys": [jwk..], "slots": {"A": kid, "B":
/// kid}}`, an empty or unbound slot `null` and left out of `keys`. The edge serves `keys` as the JWKS, and `graft-ops`'s
/// `Rotate` probe reads `slots`. A slot that holds no key is an error naming it.
pub fn published(texts: [Option<&str>; 2]) -> Result<Value, (char, KeyError)> {
    let mut keys = Vec::new();
    let mut slots = serde_json::Map::new();
    for (name, t) in ['A', 'B'].into_iter().zip(texts) {
        let kid = match t.filter(|t| !t.trim().is_empty()) {
            Some(t) => {
                let k = parse_key(t).map_err(|e| (name, e))?.to_public_key();
                let j = jwk(&k);
                let kid = j["kid"].clone();
                keys.push(j);
                kid
            }
            None => Value::Null,
        };
        slots.insert(name.to_string(), kid);
    }
    Ok(json!({ "keys": keys, "slots": slots }))
}

/// `published`'s JWKS with each key once, the same key bound in both slots listed once.
pub fn jwks(published: &Value) -> Value {
    let mut seen = std::collections::BTreeSet::new();
    let keys: Vec<Value> = published["keys"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|k| seen.insert(k["kid"].as_str().unwrap_or_default().to_owned()))
        .cloned()
        .collect();
    json!({ "keys": keys })
}

/// The issuer's `/.well-known/openid-configuration`.
pub fn discovery(issuer: &str) -> Value {
    json!({
        "issuer": issuer,
        "jwks_uri": format!("{issuer}/.well-known/jwks"),
        "subject_types_supported": ["public", "pairwise"],
        "response_types_supported": ["id_token"],
        "id_token_signing_alg_values_supported": ["RS256"],
        "scopes_supported": ["openid"],
        "claims_supported": [
            "sub", "aud", "exp", "iat", "iss", "jti", "nbf", "ref", "ref_type", "sha", "repository",
            "repository_id", "repository_owner", "repository_owner_id", "repository_visibility", "run_id",
            "run_attempt", "event_name", "workflow", "workflow_ref", "workflow_sha", "job_workflow_ref",
            "job_workflow_sha", "runner_environment", "environment", "enterprise", "enterprise_id", "head_ref",
            "base_ref",
        ],
    })
}

/// The compact JWS of `claims` under `key`: RS256, `typ` JWT, `kid` the key's thumbprint.
pub fn sign(key: &RsaPrivateKey, claims: &Value) -> String {
    use rsa::pkcs1v15::SigningKey;
    use rsa::signature::{SignatureEncoding as _, Signer as _};
    let head = json!({ "alg": "RS256", "typ": "JWT", "kid": kid(&key.to_public_key()) });
    let signed = format!("{}.{}", B64.encode(head.to_string()), B64.encode(claims.to_string()));
    let sig = SigningKey::<Sha256>::new(key.clone()).sign(signed.as_bytes()).to_vec();
    format!("{signed}.{}", B64.encode(sig))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{JobId, RunId, RunnerId};
    use crate::labels::LabelSet;
    use crate::shard::JobSpec;
    use graftyard_types::{ChainId, DbId, Index, Timestamp};
    use smallvec::SmallVec;

    fn run(event: &str, r#ref: &str, fork: bool) -> WorkflowRun {
        WorkflowRun {
            id: RunId(DbId(41)),
            repo: RepoId(DbId(74)),
            workflow: ".github/workflows/deploy.yml".into(),
            event: event.into(),
            head_sha: [0xab; 20],
            head_ref: r#ref.into(),
            trigger: (ChainId::Repo(RepoId(DbId(74))), Index(9)),
            attempt: 2,
            concurrency: None,
            fork,
        }
    }

    fn row(env: Option<&str>, state: JobState) -> JobRow {
        JobRow {
            run: RunId(DbId(41)),
            spec: JobSpec {
                id: JobId(DbId(42)),
                name: "deploy".into(),
                labels: LabelSet::parse(["ubuntu-latest"]).unwrap(),
                needs: SmallVec::new(),
                matrix: SmallVec::new(),
                environment: env.map(Into::into),
                steps: "[]".into(),
            },
            state,
            version: 3,
        }
    }

    fn running() -> JobState {
        JobState::InProgress { runner: RunnerId(DbId(5)), lease_until: Timestamp(9) }
    }

    fn grant(a: Access) -> PermissionSet {
        PermissionSet([(Box::from(ID_TOKEN_SCOPE), a), (Box::from("contents"), Access::Read)].into_iter().collect())
    }

    /// A small test key from a seeded generator, so no key file is kept; `published` and `parse_key` take a real
    /// 2,048-bit one in `a_published_slot_is_a_pkcs8_rsa_2048_key_and_its_kid_is_rfc_7638`.
    fn test_key(seed: u64, bits: usize) -> RsaPrivateKey {
        struct R(u64);
        impl rsa::rand_core::RngCore for R {
            fn next_u32(&mut self) -> u32 {
                self.next_u64() as u32
            }
            fn next_u64(&mut self) -> u64 {
                self.0 ^= self.0 << 13;
                self.0 ^= self.0 >> 7;
                self.0 ^= self.0 << 17;
                self.0
            }
            fn fill_bytes(&mut self, d: &mut [u8]) {
                for b in d {
                    *b = self.next_u64() as u8;
                }
            }
            fn try_fill_bytes(&mut self, d: &mut [u8]) -> Result<(), rsa::rand_core::Error> {
                self.fill_bytes(d);
                Ok(())
            }
        }
        impl rsa::rand_core::CryptoRng for R {}
        RsaPrivateKey::new(&mut R(seed), bits).unwrap()
    }

    fn pem(k: &RsaPrivateKey) -> String {
        use rsa::pkcs8::EncodePrivateKey as _;
        let der = k.to_pkcs8_der().unwrap();
        let b = B64S.encode(der.as_bytes());
        let lines: Vec<&str> = b.as_bytes().chunks(64).map(|c| std::str::from_utf8(c).unwrap()).collect();
        format!("-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n", lines.join("\n"))
    }

    /// GHES's examples: an environment wins over a pull request and a ref, a pull request over a ref, and a `:` in any
    /// value is `%3A`, the repository's too.
    #[test]
    fn the_legacy_sub_takes_environment_then_pull_request_then_ref() {
        assert_eq!(
            subject("octo-org/octo-repo", Some("Production"), "pull_request", "refs/pull/1/merge"),
            "repo:octo-org/octo-repo:environment:Production"
        );
        assert_eq!(
            subject("octo-org/octo-repo", None, "pull_request", "refs/pull/1/merge"),
            "repo:octo-org/octo-repo:pull_request"
        );
        assert_eq!(
            subject("octo-org/octo-repo", None, "push", "refs/heads/demo-branch"),
            "repo:octo-org/octo-repo:ref:refs/heads/demo-branch"
        );
        assert_eq!(
            subject("octo-org/octo-repo", None, "push", "refs/tags/demo-tag"),
            "repo:octo-org/octo-repo:ref:refs/tags/demo-tag"
        );
        assert_eq!(
            subject("octo-org/octo-repo", Some("Production:V1"), "push", "refs/heads/main"),
            "repo:octo-org/octo-repo:environment:Production%3AV1"
        );
        assert_eq!(subject("o/r", None, "push", "refs/heads/a:b"), "repo:o/r:ref:refs/heads/a%3Ab");
        assert_eq!(subject("o/r", None, "pull_request_target", "refs/heads/main"), "repo:o/r:pull_request");
    }

    /// Only the current attempt of a running job granted `id-token: write`, and never a fork's.
    #[test]
    fn only_a_running_granted_non_fork_attempt_mints() {
        let r = run("push", "refs/heads/main", false);
        let w = grant(Access::Write);
        assert_eq!(may_mint(&row(None, running()), &r, 2, Some(&w)), Ok(()));
        assert_eq!(may_mint(&row(None, running()), &r, 1, Some(&w)), Err(Refused::Stale));
        let assigned = JobState::Assigned { runner: RunnerId(DbId(5)), lease_until: Timestamp(9) };
        assert_eq!(may_mint(&row(None, assigned), &r, 2, Some(&w)), Err(Refused::NotRunning));
        let done = JobState::Completed { conclusion: crate::Conclusion::Success, at: Timestamp(9) };
        assert_eq!(may_mint(&row(None, done), &r, 2, Some(&w)), Err(Refused::NotRunning));
        assert_eq!(may_mint(&row(None, running()), &r, 2, None), Err(Refused::NoGrant));
        assert_eq!(may_mint(&row(None, running()), &r, 2, Some(&grant(Access::Read))), Err(Refused::NoGrant));
        assert_eq!(may_mint(&row(None, running()), &r, 2, Some(&PermissionSet::default())), Err(Refused::NoGrant));
        let fork = run("pull_request", "refs/pull/3/merge", true);
        assert_eq!(may_mint(&row(None, running()), &fork, 2, Some(&w)), Err(Refused::Fork));
        assert!(id_token_granted(Some(&w), false) && !id_token_granted(Some(&w), true));
    }

    /// Every claim of a branch push on a hosted runner, written out from GHES's reference rather than from the code.
    #[test]
    fn a_push_token_carries_ghes_claims_as_strings() {
        let (r, j) = (run("push", "refs/heads/main", false), row(None, running()));
        let f = Facts {
            server: "https://graft.example/",
            repository: "octo/hello",
            repository_id: RepoId(DbId(74)),
            owner_id: AccountId(DbId(65)),
            visibility: Visibility::Private,
            enterprise: Some(("acme", AccountId(DbId(3)))),
            run: &r,
            job: &j,
            hosted: true,
            base_ref: None,
        };
        let sha = "ab".repeat(20);
        let c = claims(&f, None, 1_000, "j-1");
        let want = json!({
            "iss": "https://graft.example/_services/token", "aud": "https://graft.example/octo",
            "sub": "repo:octo/hello:ref:refs/heads/main", "jti": "j-1", "iat": 1_000, "nbf": 400, "exp": 1_300,
            "ref": "refs/heads/main", "ref_type": "branch", "sha": sha, "repository": "octo/hello",
            "repository_id": "74", "repository_owner": "octo", "repository_owner_id": "65",
            "repository_visibility": "private", "run_id": "41", "run_attempt": "2", "event_name": "push",
            "workflow": ".github/workflows/deploy.yml",
            "workflow_ref": "octo/hello/.github/workflows/deploy.yml@refs/heads/main", "workflow_sha": sha,
            "job_workflow_ref": "octo/hello/.github/workflows/deploy.yml@refs/heads/main", "job_workflow_sha": sha,
            "runner_environment": "github-hosted", "head_ref": "", "base_ref": "",
            "enterprise": "acme", "enterprise_id": "3",
        });
        assert_eq!(c, want);
        // GitHub's example token's times (GHES 3.22, "OpenID Connect"): `nbf` 600 s before `iat`, `exp` 300 s after, so a
        // relying party whose clock lags the edge's still takes a token minted this second (F129).
        let c = claims(&f, None, 1_632_493_567, "j-1");
        assert_eq!((c["nbf"].as_u64(), c["exp"].as_u64()), (Some(1_632_492_967), Some(1_632_493_867)));
        // A custom audience replaces the owner URL; a customer's runner is self-hosted; no enterprise, no claim.
        let f = Facts { hosted: false, enterprise: None, visibility: Visibility::Public, ..f };
        let c = claims(&f, Some("sts.amazonaws.com"), 1_000, "j-2");
        assert_eq!(
            (c["aud"].as_str(), c["runner_environment"].as_str()),
            (Some("sts.amazonaws.com"), Some("self-hosted"))
        );
        assert_eq!(c["repository_visibility"], json!("public"));
        assert!(c.get("enterprise").is_none() && c.get("enterprise_id").is_none() && c.get("environment").is_none());
        // `mint` signs only what `may_mint` allows, and the token carries `claims` under the key's kid.
        let key = test_key(0x5eed, 2048);
        let w = grant(Access::Write);
        assert_eq!(mint(&f, 1, Some(&w), None, 1_000, "j", &key), Err(Refused::Stale));
        assert_eq!(mint(&f, 2, None, None, 1_000, "j", &key), Err(Refused::NoGrant));
        let t = mint(&f, 2, Some(&w), Some("sts.amazonaws.com"), 1_000, "j-3", &key).unwrap();
        let part =
            |i: usize| -> Value { serde_json::from_slice(&B64.decode(t.split('.').nth(i).unwrap()).unwrap()).unwrap() };
        assert_eq!(part(0)["kid"], json!(kid(&key.to_public_key())));
        assert_eq!(part(1), claims(&f, Some("sts.amazonaws.com"), 1_000, "j-3"));
    }

    /// A pull request's run and an environment job: the `sub`, `ref`, `environment` and `event_name` each GHES gives.
    #[test]
    fn a_pull_request_and_an_environment_job_name_their_context() {
        let (r, j) = (run("pull_request", "refs/pull/7/merge", false), row(None, running()));
        let f = Facts {
            server: "https://graft.example",
            repository: "octo/hello",
            repository_id: RepoId(DbId(74)),
            owner_id: AccountId(DbId(65)),
            visibility: Visibility::Internal,
            enterprise: None,
            run: &r,
            job: &j,
            hosted: false,
            base_ref: Some("refs/heads/rel"),
        };
        let c = claims(&f, None, 7, "x");
        assert_eq!(c["sub"], json!("repo:octo/hello:pull_request"));
        // `base_ref` is the target branch's short name, as in GitHub's pull_request tokens ("base_ref": "main"; F121).
        assert_eq!(c["base_ref"], json!("rel"));
        assert_eq!((c["ref"].as_str(), c["event_name"].as_str()), (Some("refs/pull/7/merge"), Some("pull_request")));
        assert_eq!(c["repository_visibility"], json!("internal"));
        let (r, j) = (run("push", "refs/tags/v1", false), row(Some("prod"), running()));
        let c = claims(&Facts { run: &r, job: &j, base_ref: None, ..f }, None, 7, "x");
        assert_eq!(c["sub"], json!("repo:octo/hello:environment:prod"));
        assert_eq!(c["base_ref"], json!(""));
        assert_eq!((c["environment"].as_str(), c["ref_type"].as_str()), (Some("prod"), Some("tag")));
    }

    /// RFC 7638's own example key and thumbprint (section 3.1), so `kid` is the standard's and not ours.
    #[test]
    fn the_kid_is_rfc_7638s_thumbprint() {
        let n = "0vx7agoebGcQSuuPiLJXZptN9nndrQmbXEps2aiAFbWhM78LhWx4cbbfAAtVT86zwu1RK7aPFFxuhDR1L6tSoc_BJECPebWKRXjBZCiFV4n3oknjhMstn64tZ_2W-5JsGY4Hc5n9yBXArwl93lqt7_RN5w6Cf0h4QyQ5v-65YGjQR0_FDW2QvzqY368QQMicAtaSqzs8KJZgnYb9c7d0zgdAZHzu6qMQvRL5hajrn1n91CbOpbISD08qNLyrdkt-bFTWhAI4vMQFh6WeZu0fM4lFd2NcRwr3XPksINHaQ-G_xBniIqbw0Ls1jF44-csFCur-kEgU8awapJzKnqDKgw";
        let k = RsaPublicKey::new(rsa::BigUint::from_bytes_be(&B64.decode(n).unwrap()), 65537u32.into()).unwrap();
        assert_eq!(kid(&k), "NzbLsXh8uDCcd-6MNwXF4W_7noWXFZAfHkxZsRGC9Xs");
    }

    /// The JWKS lists both slots, so a token signed under the slot a rotation switched away from still verifies; an
    /// unbound slot is left out; a slot that holds no key fails by name; a key under 2,048 bits is refused.
    #[test]
    fn a_published_slot_is_a_pkcs8_rsa_2048_key_and_its_kid_is_rfc_7638() {
        let (a, b) = (test_key(0x9e37_79b9_7f4a_7c15, 2048), test_key(0x2545_f491_4f6c_dd1d, 2048));
        let p = published([Some(&pem(&a)), Some(&pem(&b))]).unwrap();
        let (ka, kb) = (kid(&a.to_public_key()), kid(&b.to_public_key()));
        assert_ne!(ka, kb);
        assert_eq!(p["slots"], json!({ "A": ka, "B": kb }));
        let set = jwks(&p);
        assert_eq!(set["keys"].as_array().unwrap().len(), 2);
        for (k, label) in [(&a, "A"), (&b, "B")] {
            let t = sign(
                k,
                &json!({ "iss": "https://idp", "aud": "graftyard", "exp": 2_000_000_000u64, "nonce": "n",
                "sub": "s", "preferred_username": "u" }),
            );
            let ok = graftyard_identity::oidc::verify_id_token(
                &t,
                set.to_string().as_bytes(),
                "https://idp",
                "graftyard",
                "n",
                Timestamp(1_000_000),
            );
            assert!(ok.is_ok(), "slot {label}: {ok:?}");
        }
        // One slot bound, the same key in both, an unbound one.
        assert_eq!(published([Some(&pem(&a)), None]).unwrap()["slots"], json!({ "A": ka, "B": null }));
        assert_eq!(jwks(&published([Some(&pem(&a)), Some(&pem(&a))]).unwrap())["keys"].as_array().unwrap().len(), 1);
        assert_eq!(published([Some(""), Some("  ")]).unwrap()["keys"], json!([]));
        assert_eq!(published([Some(&pem(&a)), Some("hunter2")]), Err(('B', KeyError::Pem)));
        assert_eq!(parse_key(&pem(&test_key(7, 1024))).err(), Some(KeyError::Small));
        // A token under a key the JWKS does not list is refused.
        let c = test_key(0x1234_5678, 2048);
        let t = sign(
            &c,
            &json!({ "iss": "https://idp", "aud": "graftyard", "exp": 2_000_000_000u64, "nonce": "n",
            "sub": "s", "preferred_username": "u" }),
        );
        let bad = graftyard_identity::oidc::verify_id_token(
            &t,
            set.to_string().as_bytes(),
            "https://idp",
            "graftyard",
            "n",
            Timestamp(0),
        );
        assert!(bad.is_err());
    }

    #[test]
    fn discovery_names_the_issuer_and_its_jwks() {
        let d = discovery(&issuer("https://graft.example"));
        assert_eq!(d["issuer"], json!("https://graft.example/_services/token"));
        assert_eq!(d["jwks_uri"], json!("https://graft.example/_services/token/.well-known/jwks"));
        assert_eq!(d["id_token_signing_alg_values_supported"], json!(["RS256"]));
    }
}
