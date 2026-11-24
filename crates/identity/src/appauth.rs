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

/// The App an App JWT names in `iss`, before anything is verified: its id, as a number or a numeric string, or
/// `ClientId::for_app`'s client id. The edge finds the record to verify against with it.
pub fn claimed_app(jwt: &str) -> Option<AppId> {
    let p = jwt.split('.').nth(1)?;
    #[derive(Deserialize)]
    struct Issuer {
        iss: Iss,
    }
    let claims: Issuer = serde_json::from_slice(&B64.decode(p).ok()?).ok()?;
    let id = match claims.iss {
        Iss::Num(n) => n,
        Iss::Text(t) => match t.parse::<i64>() {
            Ok(n) => n,
            Err(_) => i64::from_str_radix(t.strip_prefix("Iv1.").filter(|h| h.len() == 16)?, 16).ok()?,
        },
    };
    Some(AppId(graftyard_types::DbId(id)))
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

/// A first-party App's private key from Secrets Store: PEM, PKCS#8 (`graftyard apps init` and `graftyard agents
/// init` write that) or PKCS#1 (GitHub's App key form).
pub fn app_key(pem: &str) -> Result<rsa::RsaPrivateKey, String> {
    use base64::engine::general_purpose::STANDARD;
    use rsa::pkcs1::DecodeRsaPrivateKey as _;
    use rsa::pkcs8::DecodePrivateKey as _;
    let body: String = pem.lines().map(str::trim).filter(|l| !l.starts_with("-----")).collect();
    let der = STANDARD.decode(body).map_err(|e| format!("not a PEM key: {e}"))?;
    rsa::RsaPrivateKey::from_pkcs8_der(&der)
        .or_else(|_| rsa::RsaPrivateKey::from_pkcs1_der(&der))
        .map_err(|e| format!("not an RSA private key: {e}"))
}

/// An App JWT for `app` at `now`, as GitHub's guidance asks: `iat` 60 s back for clock skew and `exp` 9 minutes on,
/// so the window is under `APP_JWT_MAX_LIFETIME_S` and `verify_app_jwt` takes it. `graft-agents-front` and the agents
/// `graft-ops` sign their `GET release/entries` reads with it (`design/AGENT-LAYER.md`, Shape, "Controller").
/// PKCS#1 v1.5 signing needs no randomness, so it runs in a Worker.
pub fn sign_app_jwt(key: &rsa::RsaPrivateKey, app: AppId, now: Timestamp) -> String {
    use rsa::pkcs1v15::SigningKey;
    use rsa::signature::{SignatureEncoding as _, Signer as _};
    let now_s = now.0 / 1000;
    let head = B64.encode(br#"{"alg":"RS256","typ":"JWT"}"#);
    let claims = B64.encode(format!(r#"{{"iat":{},"exp":{},"iss":{}}}"#, now_s - 60, now_s + 540, app.0.0));
    let signed = format!("{head}.{claims}");
    let sig = SigningKey::<Sha256>::new(key.clone()).sign(signed.as_bytes()).to_vec();
    format!("{signed}.{}", B64.encode(sig))
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

#[cfg(test)]
mod tests {
    use base64::engine::general_purpose::STANDARD;
    use rsa::pkcs1::EncodeRsaPrivateKey as _;
    use rsa::pkcs8::{EncodePrivateKey as _, EncodePublicKey as _};
    use smallvec::smallvec;

    use super::*;
    use crate::apps::{AppKey, ClientId, PermissionSet, RsaPublic, Slug};

    /// A key from a seeded generator, as `oidc`'s tests make one, so no key file is kept.
    fn key(seed: u64) -> rsa::RsaPrivateKey {
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
                d.iter_mut().for_each(|b| *b = self.next_u64() as u8);
            }
            fn try_fill_bytes(&mut self, d: &mut [u8]) -> Result<(), rsa::rand_core::Error> {
                self.fill_bytes(d);
                Ok(())
            }
        }
        impl rsa::rand_core::CryptoRng for R {}
        rsa::RsaPrivateKey::new(&mut R(seed), 1024).expect("a key")
    }

    fn record(id: i64, k: &rsa::RsaPrivateKey) -> AppRecord {
        let der = k.to_public_key().to_public_key_der().unwrap();
        AppRecord {
            id: AppId(graftyard_types::DbId(id)),
            slug: Slug::parse("graftyard-agent").unwrap(),
            name: "graftyard-agent".into(),
            client: ClientId::for_app(AppId(graftyard_types::DbId(id))),
            keys: smallvec![AppKey {
                id: graftyard_types::DbId(1),
                public: RsaPublic(der.as_bytes().into()),
                created: Timestamp(0)
            }],
            permissions: PermissionSet(Default::default()),
            events: smallvec![],
            webhook: None,
            webhook_secret: None,
            client_secrets: smallvec![],
            callbacks: smallvec![],
            public: true,
        }
    }

    /// The agents account's reader signs with the key `agents init` stored, in either PEM form, and the edge's own
    /// check takes the JWT for exactly that App, from 60 s before `now` until 9 minutes after it.
    #[test]
    fn a_signed_app_jwt_verifies_for_its_own_app_and_window_only() {
        let k = key(0x9e37_79b9_7f4a_7c15);
        let now = Timestamp(1_800_000_000_000);
        // PEM by hand: this crate builds `rsa` without its `pem` feature.
        let pem = |label: &str, der: &[u8]| {
            let b = STANDARD.encode(der);
            let lines: Vec<&str> = b.as_bytes().chunks(64).map(|c| std::str::from_utf8(c).unwrap()).collect();
            format!("-----BEGIN {label}-----\n{}\n-----END {label}-----\n", lines.join("\n"))
        };
        for pem in [
            pem("PRIVATE KEY", k.to_pkcs8_der().unwrap().as_bytes()),
            pem("RSA PRIVATE KEY", k.to_pkcs1_der().unwrap().as_bytes()),
        ] {
            assert_eq!(app_key(&pem).unwrap(), k);
        }
        assert!(app_key("-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----").is_err());
        let rec = record(327_680, &k);
        let jwt = sign_app_jwt(&k, rec.id, now);
        assert_eq!(claimed_app(&jwt), Some(rec.id));
        assert_eq!(verify_app_jwt(&jwt, &rec, now), Ok(rec.id));
        assert_eq!(verify_app_jwt(&jwt, &rec, Timestamp(now.0 + 539_000)), Ok(rec.id));
        assert_eq!(verify_app_jwt(&jwt, &rec, Timestamp(now.0 + 540_000)), Err(AppJwtError::Expired));
        assert_eq!(verify_app_jwt(&jwt, &record(393_216, &k), now), Err(AppJwtError::Issuer));
        assert_eq!(verify_app_jwt(&jwt, &record(327_680, &key(7)), now), Err(AppJwtError::Signature));
    }
}
