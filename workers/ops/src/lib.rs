//! `graft-ops`'s installation-token minting (`build/BUILD-PLAN.md`, M3-WP4; `design/DESIGN.md`, Stack, Secrets Store:
//! "the key that wraps the installation-token signing key, which puts it on the minting path"). The edge reaches it
//! only through a `fetch()`-based service binding; the script has no public route.
//!
//! `POST /mint` takes a `Grant` the edge built with `access_token_grant` and answers `{"token", "expires_at", "jti",
//! "kid"}`. The signing key is the `MintWrapKey` secret unwrapped (`MintKeySource`), held by the isolate's `Minter`
//! for at most `SIGNING_KEY_CACHE`, so a rotated secret is read within that long without a new script version.
//!
//! The rest of `graft-ops` (signed intents, the API-calling crons, `graftyard mint-key rotate`) is not built.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use graftyard_identity::Kid;
use graftyard_identity::stateless::{KeyError, KeySource, SigningSecret};
use serde::{Deserialize, Serialize};

/// The `MintWrapKey` secret's text (`design/DESIGN.md`, "Install operations", the rotation table: it "holds the wrap
/// key and the wrapped installation-token signing key"): JSON with the 32-byte wrap key, the signing key's `kid`, and
/// the 32-byte Ed25519 seed sealed under the wrap key with AES-256-GCM, as hex [inference: the design names no format].
#[derive(Clone, Serialize, Deserialize)]
pub struct MintWrapKey {
    pub wrap: String,
    pub kid: u32,
    pub nonce: String,
    pub wrapped: String,
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    s.len().is_multiple_of(2).then_some(())?;
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn bad(why: &str) -> KeyError {
    KeyError::Unwrap(why.into())
}

/// The signing key in a `MintWrapKey` secret's text, unwrapped.
pub fn unwrap(text: &str) -> Result<(Kid, SigningSecret), KeyError> {
    let m: MintWrapKey = serde_json::from_str(text).map_err(|_| bad("not a MintWrapKey"))?;
    let wrap: [u8; 32] = unhex(&m.wrap).and_then(|w| w.try_into().ok()).ok_or_else(|| bad("wrap key"))?;
    let nonce: [u8; 12] = unhex(&m.nonce).and_then(|n| n.try_into().ok()).ok_or_else(|| bad("nonce"))?;
    let ct = unhex(&m.wrapped).ok_or_else(|| bad("wrapped key"))?;
    let seed = Aes256Gcm::new(&wrap.into())
        .decrypt(Nonce::from_slice(&nonce), ct.as_slice())
        .map_err(|_| bad("the wrap key did not open the signing key"))?;
    let seed: [u8; 32] = seed.try_into().map_err(|_| bad("signing key length"))?;
    Ok((Kid(m.kid), SigningSecret(seed)))
}

/// A `MintWrapKey` secret for `seed` under `wrap`: what `graftyard mint-key rotate` writes, and the test install's
/// setup tool.
pub fn wrap(wrap: [u8; 32], kid: Kid, nonce: [u8; 12], seed: &SigningSecret) -> MintWrapKey {
    let ct = Aes256Gcm::new(&wrap.into()).encrypt(Nonce::from_slice(&nonce), seed.0.as_slice()).expect("aes-gcm seal");
    MintWrapKey { wrap: hex(&wrap), kid: kid.0, nonce: hex(&nonce), wrapped: hex(&ct) }
}

/// The key source over a `MintWrapKey` secret's text the host already read. Secrets Store reads are asynchronous, so
/// the host reads the secret only when `Minter::needs_key` says the next mint asks for a key.
pub struct MintKeySource<'a>(pub &'a str);

impl KeySource for MintKeySource<'_> {
    fn current(&self) -> Result<(Kid, SigningSecret), KeyError> {
        unwrap(self.0)
    }
}

#[cfg(target_arch = "wasm32")]
mod worker_main;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wrapped_key_unwraps_and_a_wrong_wrap_key_does_not() {
        let seed = SigningSecret([7; 32]);
        let m = wrap([1; 32], Kid(3), [2; 12], &seed);
        let text = serde_json::to_string(&m).unwrap();
        let (kid, got) = MintKeySource(&text).current().unwrap();
        assert_eq!((kid, got.0), (Kid(3), seed.0));
        assert_eq!(got.public(), seed.public());
        let wrong = MintWrapKey { wrap: hex(&[9; 32]), ..m.clone() };
        assert!(unwrap(&serde_json::to_string(&wrong).unwrap()).is_err());
        let flipped = u8::from_str_radix(&m.wrapped[..2], 16).unwrap() ^ 1;
        let tampered = MintWrapKey { wrapped: format!("{flipped:02x}{}", &m.wrapped[2..]), ..m };
        assert!(unwrap(&serde_json::to_string(&tampered).unwrap()).is_err());
        assert!(unwrap("{}").is_err());
    }
}
