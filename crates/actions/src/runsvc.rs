//! The run service's pure half (`design/SHARED.md`, "Actions", Runner protocol; `tests/m4/wp3/runner-protocol.md`):
//! what an unmodified `actions/runner` 2.338.2 is given and how what it sends is checked. The hosts (the site's
//! routes and the orb's test host in `tests/m4/wp3/`) do the I/O and commit the shard commands; everything here is
//! a function of its inputs.
//!
//! - **JIT config.** `jit_config` writes the three files the runner reads from `--jitconfig` (`.runner`,
//!   `.credentials`, `.credentials_rsaparams`) for a key pair the caller generated, with `useV2Flow`, `serverUrlV2`
//!   ending in `/`, and `IsHostedServer` false through a `gitHubUrl` on the install (SHARED, JIT configs).
//! - **The runner's OAuth exchange.** `verify_assertion` checks the RS256 (or PS256, for a FIPS runner) client
//!   assertion against `RunnerRecord.key`, with `iss = sub = clientId` and `aud` the token URL. The client id holds
//!   the owner, the registration shard, and the runner id in GUID form (`client_id`), so the edge finds the shard
//!   that holds the record with no lookup table.
//! - **Tokens.** Both the runner token and the job token are Graftyard's EdDSA JWS (`sign_token`, `verify_token`),
//!   three parts as the runner needs [measured], under a key only the run service holds. Each names the Actions shard
//!   its calls go to; the runner token names the runner (`Credential::RunnerJwt`), the job token the job and attempt
//!   (`RuntimeToken`), and its `orch_id` the run.
//! - **The job message** (`job_message`) from the run, the job's spec, and the URLs, in the runner's JSON forms.

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD as B64S, URL_SAFE_NO_PAD as B64};
use ed25519_dalek::{Signer as _, SigningKey, Verifier as _, VerifyingKey};
use graftyard_types::{AccountId, ChainId, DbId, Timestamp};
use rsa::pkcs8::DecodePublicKey;
use rsa::traits::{PrivateKeyParts, PublicKeyParts};
use rsa::{RsaPrivateKey, RsaPublicKey};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::ids::{JobId, RunnerId};
use crate::job::Conclusion;

/// The runner token's lifetime (`expires_in`) [guess: GitHub's is an hour].
pub const RUNNER_TOKEN_TTL_S: u64 = 3_600;
/// A job token outlives the longest job [doc: GitHub's job limit is 6 hours] plus the lease.
pub const JOB_TOKEN_TTL_S: u64 = 6 * 3_600 + 600;
/// An assertion's window: the runner signs `nbf` to `nbf + 300` [measured].
const ASSERTION_SKEW_S: u64 = 300;

/// The shard a runner registered on, `actions:<owner>.<shard>`, and the runner id: what its OAuth client id holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunnerAt {
    pub owner: AccountId,
    pub shard: u8,
    pub runner: RunnerId,
}

impl RunnerAt {
    pub fn chain(self) -> ChainId {
        ChainId::Actions(self.owner, self.shard)
    }
}

/// A runner's OAuth client id: GUID form ("D") of 32 hex digits, the owner id (14, ids are below 2^53), the shard
/// (2), and the runner id (16), so the exchange finds the record's shard from the assertion alone. The runner keeps
/// it as a string and never checks a GUID version [measured, `OAuthCredential.cs`].
pub fn client_id(at: RunnerAt) -> String {
    let h = format!("{:014x}{:02x}{:016x}", at.owner.0.0, at.shard, at.runner.0.0);
    format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
}

pub fn runner_of_client(c: &str) -> Option<RunnerAt> {
    let h: String = c.split('-').collect();
    if h.len() != 32 || c.len() != 36 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let n = |r: std::ops::Range<usize>| i64::from_str_radix(&h[r], 16).ok();
    Some(RunnerAt {
        owner: AccountId(DbId(n(0..14)?)),
        shard: u8::from_str_radix(&h[14..16], 16).ok()?,
        runner: RunnerId(DbId(n(16..32)?)),
    })
}

/// A job's GUID in the job message (`jobId`, `timeline.id`, `plan.planId`): the id in a fixed UUID form, its 15 hex
/// digits after the `8` variant nibble (ids are below 2^53), so the runner's later calls name it.
pub fn job_guid(kind: u8, id: i64) -> String {
    let h = format!("{id:015x}");
    format!("00000000-0000-4{kind:03x}-8{}-{}", &h[..3], &h[3..])
}

pub fn job_of_guid(g: &str) -> Option<JobId> {
    let rest = g.strip_prefix("00000000-0000-4001-8")?;
    let (a, b) = rest.split_once('-')?;
    (a.len() == 3 && b.len() == 12)
        .then(|| i64::from_str_radix(&format!("{a}{b}"), 16).ok())
        .flatten()
        .map(|n| JobId(DbId(n)))
}

/// The service's URLs, each ending in `/` [measured: the runner joins relative names onto them].
#[derive(Clone, Debug)]
pub struct Urls {
    /// The install, as `gitHubUrl` and `github.server_url`: `https://host`.
    pub server: String,
    /// `{server}/_services/`.
    pub services: String,
}

impl Urls {
    pub fn new(server: &str) -> Urls {
        let server = server.trim_end_matches('/').to_string();
        Urls { services: format!("{server}/_services/"), server }
    }
    pub fn broker(&self) -> String {
        format!("{}broker/", self.services)
    }
    pub fn run(&self) -> String {
        format!("{}run/", self.services)
    }
    pub fn results(&self) -> String {
        format!("{}results/", self.services)
    }
    pub fn token(&self) -> String {
        format!("{}oauth/token", self.services)
    }
    pub fn feed(&self, job: JobId) -> String {
        format!("{}logrelay/{}", self.services, job.0.0).replacen("http", "ws", 1)
    }
}

// ---- JIT config

fn be(n: &rsa::BigUint, len: usize) -> String {
    let b = n.to_bytes_be();
    let mut out = vec![0u8; len.saturating_sub(b.len())];
    out.extend_from_slice(&b);
    B64S.encode(out)
}

/// `.credentials_rsaparams`: each part big-endian base64, `modulus` and `d` k bytes and the CRT halves k/2, which
/// .NET's `ImportParameters` needs [measured].
fn rsa_params(k: &RsaPrivateKey) -> Value {
    let n = k.size();
    let h = n / 2;
    let (p, q) = (&k.primes()[0], &k.primes()[1]);
    let one = rsa::BigUint::from(1u8);
    let dp = k.d() % (p - &one);
    let dq = k.d() % (q - &one);
    let qinv = k.crt_coefficient().expect("two primes");
    json!({
        "d": be(k.d(), n), "dp": be(&dp, h), "dq": be(&dq, h), "exponent": B64S.encode(k.e().to_bytes_be()),
        "inverseQ": be(&qinv, h), "modulus": be(k.n(), n), "p": be(p, h), "q": be(q, h),
    })
}

/// The `--jitconfig` value for runner `id`, registered to `scope_url` (the org or repository on the install, which
/// the runner's `gitHubUrl` names), with the private key the caller generated and whose public half is the record's.
pub fn jit_config(urls: &Urls, at: RunnerAt, name: &str, scope_url: &str, key: &RsaPrivateKey) -> String {
    let runner = json!({
        "agentId": at.runner.0.0, "agentName": name, "poolId": 1, "poolName": "Default", "ephemeral": true,
        "disableUpdate": true, "gitHubUrl": scope_url, "workFolder": "_work", "useV2Flow": true,
        "serverUrlV2": urls.broker(),
    });
    let creds = json!({"scheme": "OAuth", "data": {
        "clientId": client_id(at), "authorizationUrl": urls.token(), "requireFipsCryptography": "False"}});
    let enc = |v: &Value| B64S.encode(v.to_string());
    let files =
        json!({".runner": enc(&runner), ".credentials": enc(&creds), ".credentials_rsaparams": enc(&rsa_params(key))});
    B64S.encode(files.to_string())
}

// ---- the OAuth exchange

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AssertionError {
    #[error("the assertion is not a JWT")]
    Shape,
    #[error("the assertion's algorithm is not RS256 or PS256")]
    Alg,
    #[error("the assertion does not name this runner and token URL")]
    Claims,
    #[error("the signature does not verify under the runner's key")]
    Signature,
    #[error("the assertion is outside its window")]
    Window,
}

#[derive(Deserialize)]
struct AssertionClaims {
    iss: String,
    sub: String,
    aud: String,
    nbf: u64,
    exp: u64,
}

/// The runner the assertion's `iss` names; the caller then reads its record and calls `verify_assertion`.
pub fn assertion_runner(jwt: &str) -> Option<RunnerAt> {
    let p = jwt.split('.').nth(1)?;
    let c: AssertionClaims = serde_json::from_slice(&B64.decode(p).ok()?).ok()?;
    runner_of_client(&c.iss)
}

/// Checks a runner's client assertion for runner `id`, whose record holds `key` (SPKI DER), at `now`.
pub fn verify_assertion(
    jwt: &str,
    id: RunnerAt,
    key: &[u8],
    token_url: &str,
    now: Timestamp,
) -> Result<(), AssertionError> {
    let mut parts = jwt.split('.');
    let (Some(h), Some(p), Some(s), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
        return Err(AssertionError::Shape);
    };
    let header: Value =
        B64.decode(h).ok().and_then(|b| serde_json::from_slice(&b).ok()).ok_or(AssertionError::Shape)?;
    let c: AssertionClaims =
        B64.decode(p).ok().and_then(|b| serde_json::from_slice(&b).ok()).ok_or(AssertionError::Shape)?;
    let me = client_id(id);
    if c.iss != me || c.sub != me || c.aud != token_url {
        return Err(AssertionError::Claims);
    }
    let sig = B64.decode(s).map_err(|_| AssertionError::Shape)?;
    let pk = RsaPublicKey::from_public_key_der(key).map_err(|_| AssertionError::Signature)?;
    let signed = &jwt.as_bytes()[..h.len() + 1 + p.len()];
    let ok = match header.get("alg").and_then(Value::as_str) {
        Some("RS256") => rsa::pkcs1v15::Signature::try_from(sig.as_slice())
            .is_ok_and(|g| rsa::pkcs1v15::VerifyingKey::<rsa::sha2::Sha256>::new(pk).verify(signed, &g).is_ok()),
        Some("PS256") => rsa::pss::Signature::try_from(sig.as_slice())
            .is_ok_and(|g| rsa::pss::VerifyingKey::<rsa::sha2::Sha256>::new(pk).verify(signed, &g).is_ok()),
        _ => return Err(AssertionError::Alg),
    };
    if !ok {
        return Err(AssertionError::Signature);
    }
    let t = now.0 / 1000;
    if c.nbf > t + ASSERTION_SKEW_S
        || c.exp + ASSERTION_SKEW_S <= t
        || c.exp.saturating_sub(c.nbf) > 2 * ASSERTION_SKEW_S
    {
        return Err(AssertionError::Window);
    }
    Ok(())
}

// ---- the service's tokens

/// What a token authorizes: a runner's broker and `acquirejob` calls, or one job attempt's run-service, results,
/// and live-log calls.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "k", rename_all = "lowercase")]
pub enum Grant {
    Runner { runner: RunnerId },
    Job { job: JobId, attempt: u16 },
}

#[derive(Serialize, Deserialize)]
struct TokenClaims {
    /// The Actions shard the token's calls go to.
    chain: ChainId,
    #[serde(flatten)]
    grant: Grant,
    exp: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    orch_id: Option<String>,
}

/// The service's signing key, unwrapped by the host. Never logged.
pub struct ServiceKey(pub [u8; 32]);

impl ServiceKey {
    pub fn public(&self) -> [u8; 32] {
        SigningKey::from_bytes(&self.0).verifying_key().to_bytes()
    }
}

/// A three-part EdDSA JWS for `grant` until `exp` (seconds). `orch_id` is the job token's, which the runner reads
/// for its logs [measured].
pub fn sign_token(key: &ServiceKey, chain: ChainId, grant: Grant, exp: u64, orch_id: Option<String>) -> String {
    let h = B64.encode(br#"{"typ":"JWT","alg":"EdDSA"}"#);
    let p = B64.encode(serde_json::to_vec(&TokenClaims { chain, grant, exp, orch_id }).expect("claims"));
    let signed = format!("{h}.{p}");
    let s = B64.encode(SigningKey::from_bytes(&key.0).sign(signed.as_bytes()).to_bytes());
    format!("{signed}.{s}")
}

/// The shard and grant a bearer token carries, if `public` signed it and it has not expired at `now`.
pub fn verify_token(token: &str, public: &[u8; 32], now: Timestamp) -> Option<(ChainId, Grant)> {
    let (signed, s) = token.rsplit_once('.')?;
    let p = signed.split('.').nth(1)?;
    let sig: [u8; 64] = B64.decode(s).ok()?.try_into().ok()?;
    VerifyingKey::from_bytes(public)
        .ok()?
        .verify(signed.as_bytes(), &ed25519_dalek::Signature::from_bytes(&sig))
        .ok()?;
    let c: TokenClaims = serde_json::from_slice(&B64.decode(p).ok()?).ok()?;
    (c.exp > now.0 / 1000).then_some((c.chain, c.grant))
}

// ---- runner messages and the job message

/// The broker's `RunnerJobRequest` message for job `job`: `runner_request_id` is the job's GUID, which `acquirejob`
/// sends back as `jobMessageId` [measured].
pub fn job_request(urls: &Urls, message_id: u64, job: JobId) -> Value {
    let body = json!({"id": job_guid(1, job.0.0), "runner_request_id": job_guid(1, job.0.0), "should_acknowledge": true,
                      "run_service_url": urls.run(), "billing_owner_id": ""});
    json!({"messageId": message_id, "messageType": "RunnerJobRequest", "body": body.to_string()})
}

/// The broker's `JobCancellation` message: the runner cancels the job, posts `completejob` `canceled`, and exits
/// [measured].
pub fn job_cancellation(message_id: u64, job: JobId) -> Value {
    let body = json!({"jobId": job_guid(1, job.0.0), "timeout": "00:05:00"});
    json!({"messageId": message_id, "messageType": "JobCancellation", "body": body.to_string()})
}

/// `completejob`'s `conclusion` as a job's (`succeededWithIssues` succeeded; `abandoned` is the runner losing the
/// job, a failure).
pub fn conclusion(s: &str) -> Option<Conclusion> {
    Some(match s {
        "succeeded" | "succeededWithIssues" => Conclusion::Success,
        "failed" | "abandoned" => Conclusion::Failure,
        "canceled" | "cancelled" => Conclusion::Cancelled,
        "skipped" => Conclusion::Skipped,
        _ => return None,
    })
}

/// A runner context-data dictionary from pairs, in order.
pub fn ctx_dict<'a>(pairs: impl IntoIterator<Item = (&'a str, Value)>) -> Value {
    json!({"t": 2, "d": pairs.into_iter().map(|(k, v)| json!({"k": k, "v": v})).collect::<Vec<_>>()})
}

/// The job's orchestration id, `plan.job.attempt` as GitHub's: the runner puts it in its User-Agent, so it holds
/// only token characters [measured: a job name with a space failed the worker with `FormatException`].
pub fn orch_id(run_id: i64, job: JobId, attempt: u16) -> String {
    format!("{}.{}.{attempt}", job_guid(2, run_id), job.0.0)
}

/// What a job message is built from beyond its steps.
pub struct JobMessage<'a> {
    pub job: JobId,
    pub attempt: u16,
    pub run_id: i64,
    pub job_name: &'a str,
    pub display_name: &'a str,
    /// `github.*`, as runner context data: `repository`, `sha`, `ref`, `event_name`, and the rest.
    pub github: Value,
    /// `matrix` and `strategy` as runner context data, `Null` without a matrix.
    pub matrix: Value,
    pub strategy: Value,
    /// The job's steps as the runner's `ActionStep` JSON (`steps_json`).
    pub steps: Value,
    /// The job's opened secrets: name and value, masked by the runner.
    pub secrets: Vec<(String, String)>,
    /// `vars.*`.
    pub variables: Vec<(String, String)>,
    pub github_token: &'a str,
    pub job_token: &'a str,
    pub locked_until: &'a str,
}

/// The `acquirejob` answer: the runner's `AgentJobRequestMessage` with `messageType` `RunnerJobRequest`
/// (`runner-protocol.md`, Run service).
pub fn job_message(urls: &Urls, m: &JobMessage<'_>) -> Value {
    let mut vars = serde_json::Map::new();
    vars.insert("system.github.token".into(), json!({"value": m.github_token, "isSecret": true}));
    vars.insert("system.github.job".into(), json!({"value": m.job_name}));
    vars.insert("system.github.results_endpoint".into(), json!({"value": urls.results()}));
    vars.insert("system.orchestrationId".into(), json!({"value": orch_id(m.run_id, m.job, m.attempt)}));
    for (k, v) in &m.secrets {
        vars.insert(k.clone(), json!({"value": v, "isSecret": true}));
    }
    let mut ctx = serde_json::Map::new();
    ctx.insert("github".into(), m.github.clone());
    if !m.matrix.is_null() {
        ctx.insert("matrix".into(), m.matrix.clone());
    }
    if !m.strategy.is_null() {
        ctx.insert("strategy".into(), m.strategy.clone());
    }
    ctx.insert("needs".into(), json!({"t": 2}));
    ctx.insert("inputs".into(), json!({"t": 2}));
    ctx.insert("vars".into(), ctx_dict(m.variables.iter().map(|(k, v)| (k.as_str(), Value::from(v.clone())))));
    json!({
        "messageType": "RunnerJobRequest",
        "plan": {"scopeIdentifier": job_guid(3, 0), "planType": "actions", "version": 1, "planId": job_guid(2, m.run_id)},
        "timeline": {"id": job_guid(1, m.job.0.0)},
        "jobId": job_guid(1, m.job.0.0),
        "jobDisplayName": m.display_name,
        "jobName": m.job_name,
        "requestId": m.job.0.0,
        "lockedUntil": m.locked_until,
        "resources": {"endpoints": [{
            "name": "SystemVssConnection", "url": urls.run(),
            "authorization": {"scheme": "OAuth", "parameters": {"AccessToken": m.job_token}},
            "data": {"FeedStreamUrl": urls.feed(m.job)},
        }]},
        "contextData": Value::Object(ctx),
        "variables": Value::Object(vars),
        "steps": m.steps,
        "workspace": {},
        "mask": m.secrets.iter().map(|(_, v)| json!({"type": "regex", "value": v})).collect::<Vec<_>>(),
    })
}

/// A job's steps as the runner's `ActionStep` JSON, as the runner's `PipelineTemplateConverter.ConvertToStep` builds
/// them: a `run` step is a script reference with `script`, `workingDirectory`, and `shell` inputs; a `uses` step is a
/// repository reference (`owner/repo[/path]@ref`, `./path` for the workflow's own repository) or a container
/// registry reference (`docker://image`), with `with` as its inputs. A step's GUID is its position, which is unique
/// within the job, the only scope the runner and the results service key a step by [measured].
pub fn steps_json(steps: &[crate::workflow::Step]) -> Value {
    use crate::workflow::template::{Node, T};
    let map = |pairs: Vec<(String, Value)>| json!({"type": 2, "map": pairs.into_iter().map(|(k, v)| json!({"Key": k, "Value": v})).collect::<Vec<_>>()});
    let out: Vec<Value> = steps
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let mut o = serde_json::Map::new();
            o.insert("type".into(), "action".into());
            o.insert("id".into(), format!("00000000-0000-4004-8000-{:012x}", i + 1).into());
            o.insert("name".into(), s.id.clone().into());
            o.insert("contextName".into(), s.id.clone().into());
            o.insert("condition".into(), s.cond.clone().into());
            if let Some(n) = &s.name {
                o.insert("displayNameToken".into(), n.json());
            }
            if let Some(e) = &s.env {
                o.insert("environment".into(), e.json());
            }
            if let Some(c) = &s.continue_on_error {
                o.insert("continueOnError".into(), c.json());
            }
            if let Some(t) = &s.timeout_minutes {
                o.insert("timeoutInMinutes".into(), t.json());
            }
            if let Some(run) = &s.run {
                o.insert("reference".into(), json!({"type": "script"}));
                let mut inputs = vec![("script".to_string(), run.json())];
                if let Some(w) = &s.working_directory {
                    inputs.push(("workingDirectory".into(), w.json()));
                }
                if let Some(sh) = &s.shell {
                    inputs.push(("shell".into(), sh.json()));
                }
                o.insert("inputs".into(), map(inputs));
            } else if let Some(u) = &s.uses {
                let reference = if let Some(img) = u.strip_prefix("docker://") {
                    json!({"type": "containerRegistry", "image": img})
                } else if u.starts_with("./") || u.starts_with(".\\") {
                    json!({"type": "repository", "repositoryType": "self", "path": u})
                } else {
                    let (path, r) = u.split_once('@').unwrap_or((u, ""));
                    let segs: Vec<&str> = path.split(['/', '\\']).filter(|x| !x.is_empty()).collect();
                    json!({"type": "repository", "repositoryType": "GitHub", "name": segs[..segs.len().min(2)].join("/"),
                           "ref": r, "path": segs.get(2..).map(|p| p.join("/")).unwrap_or_default()})
                };
                o.insert("reference".into(), reference);
                let with = s.with.clone().unwrap_or(Node::new(T::Map(Vec::new())));
                o.insert("inputs".into(), with.json());
            }
            Value::Object(o)
        })
        .collect();
    Value::Array(out)
}

/// Plain JSON as runner context data (`PipelineContextDataJsonConverter`), keys in serde_json's order.
pub fn context_of(v: &Value) -> Value {
    match v {
        Value::Array(a) => json!({"t": 1, "a": a.iter().map(context_of).collect::<Vec<_>>()}),
        Value::Object(m) => ctx_dict(m.iter().map(|(k, v)| (k.as_str(), context_of(v)))),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsa::pkcs1v15::SigningKey as RsSigner;
    use rsa::pkcs8::EncodePublicKey;
    use rsa::signature::{SignatureEncoding, Signer};

    /// A deterministic RNG for key generation in tests only.
    struct Rng(u64);
    impl rsa::rand_core::RngCore for Rng {
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
    impl rsa::rand_core::CryptoRng for Rng {}

    fn assertion(key: &RsaPrivateKey, iss: &str, aud: &str, nbf: u64) -> String {
        let h = B64.encode(br#"{"typ":"JWT","alg":"RS256"}"#);
        let p = B64
            .encode(json!({"iss": iss, "sub": iss, "aud": aud, "jti": "x", "nbf": nbf, "exp": nbf + 300}).to_string());
        let signed = format!("{h}.{p}");
        let s = RsSigner::<rsa::sha2::Sha256>::new(key.clone()).sign(signed.as_bytes());
        format!("{signed}.{}", B64.encode(s.to_bytes()))
    }

    #[test]
    fn a_runner_assertion_verifies_only_for_its_runner_key_url_and_window() {
        let key = RsaPrivateKey::new(&mut Rng(0x9e37_79b9_7f4a_7c15), 1024).unwrap();
        let other = RsaPrivateKey::new(&mut Rng(0x2545_f491_4f6c_dd1d), 1024).unwrap();
        let der = key.to_public_key().to_public_key_der().unwrap().as_bytes().to_vec();
        let id = RunnerAt { owner: AccountId(DbId(100)), shard: 3, runner: RunnerId(DbId(589_824)) };
        let url = "https://h/_services/oauth/token";
        let now = Timestamp(1_800_000_000_000);
        let t = now.0 / 1000;
        let good = assertion(&key, &client_id(id), url, t - 10);
        assert_eq!(assertion_runner(&good), Some(id));
        assert_eq!(verify_assertion(&good, id, &der, url, now), Ok(()));
        let cases = [
            (assertion(&other, &client_id(id), url, t), AssertionError::Signature),
            (
                assertion(&key, &client_id(RunnerAt { runner: RunnerId(DbId(589_825)), ..id }), url, t),
                AssertionError::Claims,
            ),
            (assertion(&key, &client_id(RunnerAt { shard: 4, ..id }), url, t), AssertionError::Claims),
            (assertion(&key, &client_id(id), "https://h/other", t), AssertionError::Claims),
            (assertion(&key, &client_id(id), url, t - 900), AssertionError::Window),
            (assertion(&key, &client_id(id), url, t + 600), AssertionError::Window),
        ];
        for (jwt, want) in cases {
            assert_eq!(verify_assertion(&jwt, id, &der, url, now), Err(want));
        }
        // The runner's next renewal edge: an assertion made 290 s ago, inside its 300 s window, still verifies.
        assert_eq!(verify_assertion(&assertion(&key, &client_id(id), url, t - 290), id, &der, url, now), Ok(()));
    }

    #[test]
    fn tokens_carry_their_grant_until_they_expire_and_only_under_their_key() {
        let k = ServiceKey([7; 32]);
        let g = Grant::Job { job: JobId(DbId(5)), attempt: 2 };
        let chain = ChainId::Actions(AccountId(DbId(100)), 3);
        let t = sign_token(&k, chain, g, 2_000, Some("p.5.2".into()));
        assert_eq!(t.split('.').count(), 3, "the runner parses the job token as a JWT");
        assert_eq!(verify_token(&t, &k.public(), Timestamp(1_999_000)), Some((chain, g)));
        assert_eq!(verify_token(&t, &k.public(), Timestamp(2_000_000)), None, "expired at exp");
        assert_eq!(verify_token(&t, &ServiceKey([8; 32]).public(), Timestamp(0)), None);
        // Another job's claims under this token's signature.
        let other = sign_token(&k, chain, Grant::Job { job: JobId(DbId(6)), attempt: 2 }, 2_000, None);
        let mut parts: Vec<&str> = t.split('.').collect();
        parts[1] = other.split('.').nth(1).unwrap();
        assert_eq!(verify_token(&parts.join("."), &k.public(), Timestamp(0)), None);
    }

    #[test]
    fn ids_round_trip_through_the_runner_guids() {
        // The largest ids there are: cell 255's last block (ids are below 2^53).
        let top = DbId::pack(255, DbId::MAX_BLOCK, u16::MAX).unwrap();
        let r = RunnerAt { owner: AccountId(top), shard: 15, runner: RunnerId(DbId(top.0 - 1)) };
        let g = client_id(r);
        assert_eq!((g.len(), g.matches('-').count()), (36, 4), "GUID form D");
        assert_eq!(runner_of_client(&g), Some(r));
        assert_eq!(runner_of_client(&g.replace('-', "")), None);
        let j = JobId(top);
        assert_eq!(job_of_guid(&job_guid(1, j.0.0)), Some(j));
        assert_eq!(job_of_guid(&job_guid(1, 589_826)), Some(JobId(DbId(589_826))));
        assert_eq!(job_of_guid(&job_guid(2, j.0.0)), None, "a plan GUID is not a job's");
        assert_eq!(conclusion("succeededWithIssues"), Some(Conclusion::Success));
        assert_eq!(conclusion("abandoned"), Some(Conclusion::Failure));
        assert_eq!(conclusion("canceled"), Some(Conclusion::Cancelled));
        assert_eq!(conclusion("bogus"), None);
    }
}
