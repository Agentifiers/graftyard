//! How an App and an OAuth app prove themselves (`design/SHARED.md`, "Identity and tokens"; `build/BUILD-PLAN.md`,
//! M3-WP4, BF-32).
//!
//! - **App JWT.** `POST /app/installations/{id}/access_tokens` and the other `/app` routes take a JWT the App signs
//!   with RS256: `iss` is the App id, `iat` and `exp` bound it to at most 10 minutes, and it verifies against one of
//!   `AppRecord.keys`. GitHub's guidance: "The time the JWT expires ... no more than 10 minutes into the future", and
//!   "set this 60 seconds in the past" for `iat` (`research/agents.md`, finding 4.2) [doc for github.com; unverified
//!   for GHES 3.22].
//! - **Client secret.** An OAuth app's or an App's `client_secret` is checked by SHA-256 against `client_secrets`,
//!   which hold no secret, only hashes. A first-party client's embedded secret is ignored; the edge does not call
//!   this for one.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use graftyard_types::Timestamp;
use rsa::RsaPublicKey;
use rsa::pkcs1v15::{Signature, VerifyingKey};
use rsa::pkcs8::DecodePublicKey;
use rsa::sha2::Sha256;
use rsa::signature::Verifier;
use serde::Deserialize;
use sha2::Digest as _;

use crate::apps::{AppId, AppRecord, SecretHash};

/// The longest an App JWT may live, `exp - iat` [doc: GitHub's 10 minutes].
pub const APP_JWT_MAX_LIFETIME_S: u64 = 600;
/// How far `iat` may be ahead of the verifier's clock, as `crate::stateless::CLOCK_SKEW` [guess].
pub const APP_JWT_SKEW_S: u64 = 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AppJwtError {
    #[error("not a compact JWS")]
    Shape,
    #[error("the JWS header is not RS256")]
    Header,
    #[error("the claims do not parse")]
    Claims,
    #[error("iss is not the App")]
    Issuer,
    #[error("no key of the App verifies the signature")]
    Signature,
    #[error("expired")]
    Expired,
    #[error("issued in the future")]
    NotYet,
    #[error("lives longer than 10 minutes")]
    Lifetime,
}

#[derive(Deserialize)]
struct Header {
    alg: String,
}

/// `iss` is a number or a numeric string: Octokit sends the App id as a number, and some clients send the client id.
#[derive(Deserialize)]
#[serde(untagged)]
enum Iss {
    Num(i64),
    Text(String),
}

#[derive(Deserialize)]
struct Claims {
    iss: Iss,
    iat: u64,
    exp: u64,
}

/// Verifies an App JWT for `record` at `now`: RS256 under one of the record's keys, `iss` the App's id or client
/// id, and a window of at most 10 minutes that contains `now`. Returns the App's id.
pub fn verify_app_jwt(jwt: &str, record: &AppRecord, now: Timestamp) -> Result<AppId, AppJwtError> {
    let mut parts = jwt.split('.');
    let (Some(h), Some(p), Some(s), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
        return Err(AppJwtError::Shape);
    };
    let header: Header = B64.decode(h).ok().and_then(|b| serde_json::from_slice(&b).ok()).ok_or(AppJwtError::Header)?;
    if header.alg != "RS256" {
        return Err(AppJwtError::Header);
    }
    let claims: Claims = B64.decode(p).ok().and_then(|b| serde_json::from_slice(&b).ok()).ok_or(AppJwtError::Claims)?;
    let ours = match &claims.iss {
        Iss::Num(n) => *n == record.id.0.0,
        Iss::Text(t) => t.parse::<i64>().is_ok_and(|n| n == record.id.0.0) || t == record.client.as_str(),
    };
    if !ours {
        return Err(AppJwtError::Issuer);
    }
    let sig = B64.decode(s).map_err(|_| AppJwtError::Shape)?;
    let sig = Signature::try_from(sig.as_slice()).map_err(|_| AppJwtError::Signature)?;
    let signed = &jwt.as_bytes()[..h.len() + 1 + p.len()];
    let verified = record.keys.iter().any(|k| {
        RsaPublicKey::from_public_key_der(&k.public.0)
            .is_ok_and(|key| VerifyingKey::<Sha256>::new(key).verify(signed, &sig).is_ok())
    });
    if !verified {
        return Err(AppJwtError::Signature);
    }
    let now_s = now.0 / 1000;
    if claims.exp.saturating_sub(claims.iat) > APP_JWT_MAX_LIFETIME_S {
        return Err(AppJwtError::Lifetime);
    }
    if claims.iat > now_s + APP_JWT_SKEW_S {
        return Err(AppJwtError::NotYet);
    }
    if claims.exp <= now_s {
        return Err(AppJwtError::Expired);
    }
    Ok(record.id)
}

/// SHA-256 of a client secret: the only form a record holds.
pub fn secret_hash(secret: &str) -> SecretHash {
    SecretHash(sha2::Sha256::digest(secret.as_bytes()).into())
}

/// Whether `secret` is one of the record's client secrets. A wrong or missing secret is the edge's 401.
pub fn client_secret_ok(secrets: &[SecretHash], secret: &str) -> bool {
    let h = secret_hash(secret);
    // Every hash is compared, so the time does not say which one matched.
    secrets.iter().fold(false, |ok, s| ok | (s.0 == h.0))
}
