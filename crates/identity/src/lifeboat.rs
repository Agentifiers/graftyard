//! A lifeboat's signed requests to the export endpoints (`design/DESIGN.md`, Shape, "Lifeboat feed"; "Install
//! operations", Lifeboat; `decisions/DECISIONS.md`, "`lifeboat-auth`").
//!
//! A lifeboat pulls log tails, snapshots, and packs through the edge when the R2 feed is stale. It holds no export
//! token: it signs each request with the key its `LifeboatAdd` registered, which signs nothing else but its
//! `LifeboatDecoder`. The request carries three headers:
//!
//! - `X-Graftyard-Lifeboat: <host>`, the host its `LifeboatAdd` named;
//! - `X-Graftyard-Lifeboat-At: <ms>`, the request's time;
//! - `X-Graftyard-Lifeboat-Sig: <base64url signature>`, Ed25519 over `REQUEST_DOMAIN`, the host, the time, the method,
//!   and the path with its query, each length-prefixed, so no two requests sign the same bytes.
//!
//! The edge checks the signature against the `Release` view's `lifeboats` row for that host, so a removed lifeboat
//! is refused as soon as the view holds its `LifeboatRemove`, and accepts a time within `CLOCK_SKEW` of its own. A
//! signed request covers what an install-scoped export covers, but never the feed's batch listing, which the lifeboat
//! reads from R2 itself, and never a token revocation. A replay inside the skew can only fetch the same bytes again.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use ed25519_dalek::{Signer as _, SigningKey, VerifyingKey};
use graftyard_release::KeyId;
use graftyard_types::Timestamp;

use crate::stateless::CLOCK_SKEW;

pub const HOST_HEADER: &str = "x-graftyard-lifeboat";
pub const AT_HEADER: &str = "x-graftyard-lifeboat-at";
pub const SIG_HEADER: &str = "x-graftyard-lifeboat-sig";
/// What every signed request starts with, so a signature over one is never one over a `LifeboatDecoder`.
pub const REQUEST_DOMAIN: &[u8] = b"graftyard/lifeboat/request/v1";
/// The longest host a request may name, as a DNS name is.
pub const HOST_MAX: usize = 253;

/// One signed request's headers, as sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedRequest {
    pub host: String,
    pub at: Timestamp,
    pub sig: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum LifeboatAuthError {
    #[error("a lifeboat header is missing or malformed")]
    Shape,
    #[error("no lifeboat by that host is registered")]
    Unknown,
    #[error("the request's time is outside the clock skew")]
    Skew,
    #[error("bad signature")]
    Signature,
}

fn message(host: &str, at: Timestamp, method: &str, path_and_query: &str) -> Vec<u8> {
    let mut m = Vec::with_capacity(64 + host.len() + path_and_query.len());
    m.extend_from_slice(REQUEST_DOMAIN);
    for part in [host.as_bytes(), &at.0.to_be_bytes(), method.as_bytes(), path_and_query.as_bytes()] {
        m.extend_from_slice(&(part.len() as u32).to_be_bytes());
        m.extend_from_slice(part);
    }
    m
}

/// The headers for a request, signed with the lifeboat key's seed: what the lifeboat sends.
pub fn sign(seed: &[u8; 32], host: &str, at: Timestamp, method: &str, path_and_query: &str) -> SignedRequest {
    let sig = SigningKey::from_bytes(seed).sign(&message(host, at, method, path_and_query));
    SignedRequest { host: host.to_owned(), at, sig: B64.encode(sig.to_bytes()) }
}

/// Checks a request's lifeboat headers at `now`: the time within `CLOCK_SKEW`, and the signature under the key
/// `registered` returns for the host (the `Release` view's `lifeboats`). Returns the host.
pub fn verify<'a>(
    host: Option<&'a str>,
    at: Option<&str>,
    sig: Option<&str>,
    method: &str,
    path_and_query: &str,
    now: Timestamp,
    registered: impl Fn(&str) -> Option<KeyId>,
) -> Result<&'a str, LifeboatAuthError> {
    let (Some(host), Some(at), Some(sig)) = (host, at, sig) else { return Err(LifeboatAuthError::Shape) };
    if host.is_empty() || host.len() > HOST_MAX {
        return Err(LifeboatAuthError::Shape);
    }
    let at = Timestamp(at.parse().map_err(|_| LifeboatAuthError::Shape)?);
    let sig: [u8; 64] = B64.decode(sig).ok().and_then(|b| b.try_into().ok()).ok_or(LifeboatAuthError::Shape)?;
    let key = registered(host).ok_or(LifeboatAuthError::Unknown)?;
    if at.0.abs_diff(now.0) > CLOCK_SKEW.as_millis() as u64 {
        return Err(LifeboatAuthError::Skew);
    }
    let vk = VerifyingKey::from_bytes(&key.0).map_err(|_| LifeboatAuthError::Signature)?;
    vk.verify_strict(&message(host, at, method, path_and_query), &ed25519_dalek::Signature::from_bytes(&sig))
        .map_err(|_| LifeboatAuthError::Signature)?;
    Ok(host)
}

#[cfg(test)]
mod tests {
    use super::*;
    use graftyard_release::public_key;

    const SEED: [u8; 32] = [7; 32];
    const NOW: Timestamp = Timestamp(1_800_000_000_000);
    const PATH: &str = "/api/graftyard/v1/export/chains/repo:5/entries?from=10&limit=100";

    fn check(r: &SignedRequest, method: &str, path: &str, now: Timestamp) -> Result<(), LifeboatAuthError> {
        let reg = |h: &str| (h == "lb.acme.example").then(|| public_key(&SEED));
        verify(Some(&r.host), Some(&r.at.0.to_string()), Some(&r.sig), method, path, now, reg).map(|_| ())
    }

    #[test]
    fn a_signed_request_verifies_only_as_signed_within_the_skew() {
        let r = sign(&SEED, "lb.acme.example", NOW, "GET", PATH);
        assert_eq!(check(&r, "GET", PATH, NOW), Ok(()));
        let skew = CLOCK_SKEW.as_millis() as u64;
        assert_eq!(check(&r, "GET", PATH, Timestamp(NOW.0 + skew)), Ok(()));
        assert_eq!(check(&r, "GET", PATH, Timestamp(NOW.0 - skew)), Ok(()));
        assert_eq!(check(&r, "GET", PATH, Timestamp(NOW.0 + skew + 1)), Err(LifeboatAuthError::Skew));
        assert_eq!(check(&r, "GET", PATH, Timestamp(NOW.0 - skew - 1)), Err(LifeboatAuthError::Skew));
        // Another path, query, method, or time is another message.
        let other = PATH.replace("from=10", "from=11");
        assert_eq!(check(&r, "GET", &other, NOW), Err(LifeboatAuthError::Signature));
        assert_eq!(check(&r, "DELETE", PATH, NOW), Err(LifeboatAuthError::Signature));
        let moved = SignedRequest { at: Timestamp(NOW.0 + 1), ..r.clone() };
        assert_eq!(check(&moved, "GET", PATH, NOW), Err(LifeboatAuthError::Signature));
    }

    #[test]
    fn another_key_or_an_unregistered_host_is_refused() {
        let wrong = sign(&[8; 32], "lb.acme.example", NOW, "GET", PATH);
        assert_eq!(check(&wrong, "GET", PATH, NOW), Err(LifeboatAuthError::Signature));
        // A removed lifeboat: the view no longer names its host.
        let r = sign(&SEED, "lb2.acme.example", NOW, "GET", PATH);
        assert_eq!(check(&r, "GET", PATH, NOW), Err(LifeboatAuthError::Unknown));
        assert_eq!(
            verify(None, Some("1"), Some("x"), "GET", PATH, NOW, |_| Some(public_key(&SEED))),
            Err(LifeboatAuthError::Shape)
        );
    }

    /// Two fields that would run together without the length prefixes sign different bytes.
    #[test]
    fn fields_do_not_run_together() {
        assert_ne!(message("ab", NOW, "GET", "/c"), message("a", NOW, "bGET", "/c"));
    }
}
