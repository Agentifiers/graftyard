//! The OIDC sign-in's checks (`design/SHARED.md`, "Instance sign-in in installs"; `build/BUILD-PLAN.md`, M3-WP3,
//! BF-36): the edge sends the browser to the issuer's authorization endpoint with a `state`, a `nonce`, and a PKCE
//! challenge, takes the code back, exchanges it at the token endpoint, and signs the user in only if the `id_token`
//! verifies here. The edge then commits `IdentityOp::SignIn { method: Oidc }`, which the `Identity` leader checks
//! against `InstanceAuth`.
//!
//! An `id_token` is accepted only with RS256 under a key of the issuer's JWKS (by `kid`, or the only RSA key), `iss`
//! the configured issuer, `aud` naming the configured client, the `nonce` the edge sent, and `exp` in the future
//! [doc: OpenID Connect Core 1.0, 3.1.3.7, ID Token Validation]. The account is the one whose login is the token's
//! `preferred_username` [inference: SHARED says an install's OIDC "maps each identity to its imported login"].

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use graftyard_types::Timestamp;
use rsa::pkcs1v15::{Signature, VerifyingKey};
use rsa::sha2::Sha256;
use rsa::signature::Verifier;
use rsa::{BigUint, RsaPublicKey};
use serde::Deserialize;
use sha2::Digest as _;

/// How far `iat` may be ahead of the edge's clock [guess, as `crate::stateless::CLOCK_SKEW`].
pub const OIDC_SKEW_S: u64 = 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum OidcError {
    #[error("not a compact JWS")]
    Shape,
    #[error("the JWS header is not RS256")]
    Header,
    #[error("the claims do not parse")]
    Claims,
    #[error("no key of the issuer's JWKS verifies the signature")]
    Signature,
    #[error("iss is not the configured issuer")]
    Issuer,
    #[error("aud does not name the configured client")]
    Audience,
    #[error("the nonce is not the one this sign-in sent")]
    Nonce,
    #[error("expired")]
    Expired,
    #[error("issued in the future")]
    NotYet,
    #[error("no preferred_username")]
    NoUsername,
}

#[derive(Deserialize)]
struct Header {
    alg: String,
    #[serde(default)]
    kid: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Aud {
    One(String),
    Many(Vec<String>),
}

#[derive(Deserialize)]
struct Claims {
    iss: String,
    aud: Aud,
    exp: u64,
    #[serde(default)]
    iat: Option<u64>,
    #[serde(default)]
    nonce: Option<String>,
    sub: String,
    #[serde(default)]
    preferred_username: Option<String>,
}

#[derive(Deserialize)]
struct Jwk {
    kty: String,
    #[serde(default)]
    kid: Option<String>,
    #[serde(default)]
    n: Option<String>,
    #[serde(default)]
    e: Option<String>,
}

#[derive(Deserialize)]
struct Jwks {
    keys: Vec<Jwk>,
}

/// The identity a verified `id_token` names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OidcIdentity {
    pub sub: String,
    pub username: String,
}

/// The `nonce` the edge sends for a sign-in whose `state` is `state`: the SHA-256 of it in hex, so the callback can
/// check the token's nonce from the state alone.
pub fn nonce_for(state: &str) -> String {
    sha2::Sha256::digest(state.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// The PKCE `S256` challenge of `verifier` [doc: RFC 7636, 4.2].
pub fn pkce_challenge(verifier: &str) -> String {
    B64.encode(sha2::Sha256::digest(verifier.as_bytes()))
}

fn key(j: &Jwk) -> Option<RsaPublicKey> {
    let n = BigUint::from_bytes_be(&B64.decode(j.n.as_deref()?).ok()?);
    let e = BigUint::from_bytes_be(&B64.decode(j.e.as_deref()?).ok()?);
    RsaPublicKey::new(n, e).ok()
}

/// Verifies `id_token` against the issuer's JWKS document `jwks` (JSON), for `issuer`, `client`, and the `nonce`
/// this sign-in sent, at `now`.
pub fn verify_id_token(
    id_token: &str,
    jwks: &[u8],
    issuer: &str,
    client: &str,
    nonce: &str,
    now: Timestamp,
) -> Result<OidcIdentity, OidcError> {
    let mut parts = id_token.split('.');
    let (Some(h), Some(p), Some(s), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
        return Err(OidcError::Shape);
    };
    let header: Header = B64.decode(h).ok().and_then(|b| serde_json::from_slice(&b).ok()).ok_or(OidcError::Header)?;
    if header.alg != "RS256" {
        return Err(OidcError::Header);
    }
    let jwks: Jwks = serde_json::from_slice(jwks).map_err(|_| OidcError::Signature)?;
    let rsa: Vec<&Jwk> = jwks.keys.iter().filter(|k| k.kty == "RSA").collect();
    // By `kid` when the header names one; else only a JWKS with one RSA key says which key signed.
    let chosen: Vec<&Jwk> = match &header.kid {
        Some(kid) => rsa.into_iter().filter(|k| k.kid.as_deref() == Some(kid.as_str())).collect(),
        None if rsa.len() == 1 => rsa,
        None => Vec::new(),
    };
    let sig = B64.decode(s).map_err(|_| OidcError::Shape)?;
    let sig = Signature::try_from(sig.as_slice()).map_err(|_| OidcError::Signature)?;
    let signed = &id_token.as_bytes()[..h.len() + 1 + p.len()];
    let verified =
        chosen.iter().filter_map(|k| key(k)).any(|k| VerifyingKey::<Sha256>::new(k).verify(signed, &sig).is_ok());
    if !verified {
        return Err(OidcError::Signature);
    }
    let c: Claims = B64.decode(p).ok().and_then(|b| serde_json::from_slice(&b).ok()).ok_or(OidcError::Claims)?;
    if c.iss != issuer {
        return Err(OidcError::Issuer);
    }
    let ours = match &c.aud {
        Aud::One(a) => a == client,
        Aud::Many(v) => v.iter().any(|a| a == client),
    };
    if !ours {
        return Err(OidcError::Audience);
    }
    if c.nonce.as_deref() != Some(nonce) {
        return Err(OidcError::Nonce);
    }
    let now_s = now.0 / 1000;
    if c.exp <= now_s {
        return Err(OidcError::Expired);
    }
    if c.iat.is_some_and(|iat| iat > now_s + OIDC_SKEW_S) {
        return Err(OidcError::NotYet);
    }
    let username = c.preferred_username.filter(|u| !u.is_empty()).ok_or(OidcError::NoUsername)?;
    Ok(OidcIdentity { sub: c.sub, username })
}

#[cfg(test)]
mod tests {
    use rsa::RsaPrivateKey;
    use rsa::pkcs1v15::SigningKey;
    use rsa::signature::{SignatureEncoding, Signer};
    use rsa::traits::PublicKeyParts;
    use serde_json::json;

    use super::*;

    /// A test key from a seeded generator, so no key file is kept: `rsa`'s `new` takes any `CryptoRngCore`.
    fn test_key(seed: u64) -> RsaPrivateKey {
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
        RsaPrivateKey::new(&mut R(seed), 1024).expect("a key")
    }

    fn jwks(k: &RsaPrivateKey, kid: &str) -> Vec<u8> {
        let p = k.to_public_key();
        json!({ "keys": [{ "kty": "RSA", "kid": kid, "alg": "RS256", "use": "sig",
            "n": B64.encode(p.n().to_bytes_be()), "e": B64.encode(p.e().to_bytes_be()) }] })
        .to_string()
        .into_bytes()
    }

    fn token(k: &RsaPrivateKey, kid: &str, claims: serde_json::Value) -> String {
        let h = B64.encode(json!({ "alg": "RS256", "kid": kid, "typ": "JWT" }).to_string());
        let p = B64.encode(claims.to_string());
        let sig = SigningKey::<Sha256>::new(k.clone()).sign(format!("{h}.{p}").as_bytes());
        format!("{h}.{p}.{}", B64.encode(sig.to_bytes()))
    }

    #[test]
    fn an_id_token_is_accepted_only_with_its_key_issuer_audience_nonce_and_time() {
        let k = test_key(0x9e37_79b9_7f4a_7c15);
        let set = jwks(&k, "k1");
        let now = Timestamp(1_800_000_000_000);
        let n = nonce_for("st");
        let good = json!({ "iss": "https://idp", "aud": "graftyard", "exp": 1_800_000_600u64, "iat": 1_800_000_000u64,
            "nonce": n, "sub": "u-1", "preferred_username": "m3-admin" });
        let ok = verify_id_token(&token(&k, "k1", good.clone()), &set, "https://idp", "graftyard", &n, now);
        assert_eq!(ok, Ok(OidcIdentity { sub: "u-1".into(), username: "m3-admin".into() }));
        let with = |f: &str, v: serde_json::Value| {
            let mut c = good.clone();
            c[f] = v;
            verify_id_token(&token(&k, "k1", c), &set, "https://idp", "graftyard", &n, now)
        };
        assert_eq!(with("iss", json!("https://other")), Err(OidcError::Issuer));
        assert_eq!(with("aud", json!(["x", "y"])), Err(OidcError::Audience));
        assert_eq!(with("nonce", json!(nonce_for("other"))), Err(OidcError::Nonce));
        assert_eq!(with("exp", json!(1_800_000_000u64)), Err(OidcError::Expired));
        assert_eq!(with("iat", json!(1_800_000_100u64)), Err(OidcError::NotYet));
        assert_eq!(with("preferred_username", json!("")), Err(OidcError::NoUsername));
        assert!(with("aud", json!(["x", "graftyard"])).is_ok());
        // Another key's signature, an unknown kid, and a changed payload.
        let other = test_key(0x2545_f491_4f6c_dd1d);
        let t = token(&other, "k1", good.clone());
        assert_eq!(verify_id_token(&t, &set, "https://idp", "graftyard", &n, now), Err(OidcError::Signature));
        let t = token(&k, "k2", good.clone());
        assert_eq!(verify_id_token(&t, &set, "https://idp", "graftyard", &n, now), Err(OidcError::Signature));
        let t = token(&k, "k1", good);
        let mut parts: Vec<String> = t.split('.').map(str::to_owned).collect();
        parts[1] = B64.encode(
            json!({ "iss": "https://idp", "aud": "graftyard", "exp": 1_800_000_600u64, "nonce": n,
            "sub": "u-2", "preferred_username": "m3-member" })
            .to_string(),
        );
        assert_eq!(
            verify_id_token(&parts.join("."), &set, "https://idp", "graftyard", &n, now),
            Err(OidcError::Signature)
        );
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }
}
