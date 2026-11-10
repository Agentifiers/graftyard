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
use graftyard_identity::sealed::{HookSecret, HookSecrets, Wrapped};
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

/// One `SecretsWrapKey` slot's text (`design/DESIGN.md`, Shape, `SecretName`; Secrets Store's two slots per rotatable
/// secret): JSON with the slot number and the 32-byte AES-256 key as hex [inference: the design names no format].
#[derive(Clone, Serialize, Deserialize)]
pub struct WrapKeySlot {
    pub slot: u8,
    pub key: String,
}

/// `HookSecrets` over the `SecretsWrapKey` slots the host read: `current` wraps, and any slot listed unwraps what it
/// wrapped. A hook secret never leaves this crate's `unwrap` except as the HMAC key `HookSequencer` signs with.
pub struct WrapKeySource {
    pub current: u8,
    pub slots: Vec<(u8, [u8; 32])>,
}

impl WrapKeySource {
    /// From the slot texts the host read; `current` names the slot new values wrap under.
    pub fn parse(current: u8, texts: &[&str]) -> Result<WrapKeySource, KeyError> {
        let mut slots = Vec::new();
        for t in texts {
            let s: WrapKeySlot = serde_json::from_str(t).map_err(|_| bad("not a SecretsWrapKey slot"))?;
            let k: [u8; 32] = unhex(&s.key).and_then(|k| k.try_into().ok()).ok_or_else(|| bad("wrap key"))?;
            slots.push((s.slot, k));
        }
        Ok(WrapKeySource { current, slots })
    }

    fn key(&self, slot: u8) -> Result<Aes256Gcm, KeyError> {
        let k = self.slots.iter().find(|(s, _)| *s == slot).ok_or_else(|| bad("no such wrap key slot"))?.1;
        Ok(Aes256Gcm::new(&k.into()))
    }
}

impl HookSecrets for WrapKeySource {
    fn wrap(&self, plain: &[u8], nonce: [u8; 12]) -> Result<Wrapped, KeyError> {
        let ct = self.key(self.current)?.encrypt(Nonce::from_slice(&nonce), plain).map_err(|_| bad("seal"))?;
        Ok(Wrapped { slot: self.current, nonce, ciphertext: ct.into() })
    }

    fn unwrap(&self, w: &Wrapped) -> Result<HookSecret, KeyError> {
        let plain = self
            .key(w.slot)?
            .decrypt(Nonce::from_slice(&w.nonce), w.ciphertext.as_ref())
            .map_err(|_| bad("the wrap key did not open the hook secret"))?;
        Ok(HookSecret(plain.into()))
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

    #[test]
    fn a_hook_secret_wraps_and_unwraps_under_its_slot_and_signs() {
        let slot = |s: u8, k: u8| serde_json::to_string(&WrapKeySlot { slot: s, key: hex(&[k; 32]) }).unwrap();
        let (one, two) = (slot(1, 4), slot(2, 5));
        let old = WrapKeySource::parse(1, &[&one]).unwrap();
        let w = old.wrap(b"It's a Secret to Everybody", [3; 12]).unwrap();
        assert_eq!(w.slot, 1);
        assert!(!w.ciphertext.windows(6).any(|x| x == b"Secret"));
        // After a rotation to slot 2, a value wrapped under slot 1 still opens while slot 1 is listed.
        let both = WrapKeySource::parse(2, &[&one, &two]).unwrap();
        let s = both.unwrap(&w).unwrap();
        assert_eq!(
            graftyard_identity::sealed::signature_256(&s, b"Hello, World!"),
            "sha256=757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17"
        );
        assert_eq!(both.wrap(b"x", [0; 12]).unwrap().slot, 2);
        // Slot 1 cleared: the old value no longer opens. A tampered value never does.
        assert!(WrapKeySource::parse(2, &[&two]).unwrap().unwrap(&w).is_err());
        let mut t = w.clone();
        t.ciphertext[0] ^= 1;
        assert!(both.unwrap(&t).is_err());
    }
}
