//! Wrapped secrets and webhook signing (`design/SHARED.md`, \"Actions\", Secrets and variables, and \"Event fan-out\";
//! `design/DESIGN.md`, Shape, `SecretName::SecretsWrapKey`).
//!
//! - `Wrapped` is SHARED's: AES-256-GCM under the `SecretsWrapKey` slot that `slot` names. A hook's `secret` is stored
//!   only so (`HookRecord.secret`), and only webhook signing unwraps it.
//! - `HookSecrets` is the `graft-ops` interface: \"Only `graft-ops` reads `SecretsWrapKey`: it ... unwraps hook secrets
//!   for `HookSequencer`\". `graft-ops` implements it over the Secrets Store slots (`graftyard_ops::WrapKeySource`); a
//!   test stubs it in memory, as M3-WP4 stubbed `KeySource`.
//! - `signature_256` is `X-Hub-Signature-256`: `sha256=` and the hex HMAC-SHA256 of the body under the secret
//!   (`research/wire-census.md` #36).

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::stateless::KeyError;

/// SHARED's `Wrapped`. The ciphertext includes GCM's tag. `Debug` prints its length only.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Wrapped {
    pub slot: u8,
    pub nonce: [u8; 12],
    pub ciphertext: Box<[u8]>,
}

impl std::fmt::Debug for Wrapped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Wrapped {{ slot: {}, {} bytes }}", self.slot, self.ciphertext.len())
    }
}

/// A hook's secret, unwrapped. Never printed, never stored.
#[derive(Clone, PartialEq, Eq)]
pub struct HookSecret(pub Box<[u8]>);

impl std::fmt::Debug for HookSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HookSecret(..)")
    }
}

/// `graft-ops`'s wrap and unwrap under `SecretsWrapKey`. `wrap` seals a secret the client sent under the current slot
/// with the host's random `nonce`; `unwrap` opens one under the slot it names, so a value wrapped before a rotation
/// opens until the rotation rewraps it.
pub trait HookSecrets {
    fn wrap(&self, plain: &[u8], nonce: [u8; 12]) -> Result<Wrapped, KeyError>;
    fn unwrap(&self, w: &Wrapped) -> Result<HookSecret, KeyError>;
}

/// HMAC-SHA256 (RFC 2104) of `body` under `key`.
pub fn hmac_sha256(key: &[u8], body: &[u8]) -> [u8; 32] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let pad = |b: u8| k.map(|x| x ^ b);
    let inner = Sha256::new().chain_update(pad(0x36)).chain_update(body).finalize();
    Sha256::new().chain_update(pad(0x5c)).chain_update(inner).finalize().into()
}

/// The `X-Hub-Signature-256` value for `body`.
pub fn signature_256(secret: &HookSecret, body: &[u8]) -> String {
    let mac = hmac_sha256(&secret.0, body);
    let mut s = String::with_capacity(71);
    s.push_str("sha256=");
    for b in mac {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// GitHub's documented example (docs.github.com, \"Validating webhook deliveries\", Testing the webhook payload
    /// validation) and RFC 4231 test case 2.
    #[test]
    fn signature_matches_githubs_example_and_rfc_4231() {
        let s = HookSecret(b"It's a Secret to Everybody".to_vec().into());
        assert_eq!(
            signature_256(&s, b"Hello, World!"),
            "sha256=757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17"
        );
        let mac = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
        let hex: String = mac.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
        // A key longer than the block is hashed first (RFC 4231 test case 6).
        let mac = hmac_sha256(&[0xaa; 131], b"Test Using Larger Than Block-Size Key - Hash Key First");
        let hex: String = mac.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54");
    }

    #[test]
    fn debug_prints_no_secret() {
        let s = HookSecret(b"hunter2".to_vec().into());
        assert_eq!(format!("{s:?}"), "HookSecret(..)");
        let w = Wrapped { slot: 1, nonce: [0; 12], ciphertext: b"hunter2".to_vec().into() };
        assert!(!format!("{w:?}").contains("hunter2") && !format!("{w:?}").contains("104"));
    }
}
