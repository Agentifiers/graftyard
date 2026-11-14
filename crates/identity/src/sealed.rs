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

// ---- Actions secrets and variables (`design/SHARED.md`, "Actions", Secrets and variables). Wire tags: add variants
// at the end only.

/// SHARED's `SealKey`: one X25519 key pair per owner, whose `public` `gh secret set` seals to with
/// `crypto_box_seal`. `key_id` is the decimal key id GitHub's clients send back. `private` is the X25519 secret
/// wrapped under `SecretsWrapKey`; only `graft-ops` opens it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SealKey {
    pub key_id: Box<str>,
    pub public: [u8; 32],
    pub private: Wrapped,
    pub created: graftyard_types::Timestamp,
    pub retired: Option<graftyard_types::Timestamp>,
}

/// SHARED's `ActionsName`: upper-case letters, digits, and underscores, not starting with a digit or `GITHUB_`.
/// GitHub stores names upper-cased, so `parse` upper-cases [unverified as GitHub's exact rule].
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ActionsName(Box<str>);

impl ActionsName {
    pub fn parse(s: &str) -> Option<ActionsName> {
        let up = s.to_ascii_uppercase();
        let ok = !up.is_empty()
            && up.len() <= 255
            && up.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
            && !up.as_bytes()[0].is_ascii_digit()
            && !up.starts_with("GITHUB_");
        ok.then(|| ActionsName(up.into()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A deployment environment's name, as the workflow's `environment:` and the REST path give it. GitHub matches
/// environment names case-insensitively [unverified]; they are kept as given and compared as given here.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EnvName(pub Box<str>);

/// SHARED's `SecretVisibility`, an org secret's or variable's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum SecretVisibility {
    All,
    Private,
    Selected,
}

/// SHARED's `SecretScope`. A repository chain carries `Repo` and `Env`; an owner's `Authz` chain carries `Org`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SecretScope {
    Repo,
    Env(EnvName),
    Org { visibility: SecretVisibility, repos: smallvec::SmallVec<[graftyard_types::RepoId; 4]> },
}

impl SecretScope {
    /// Whether a job of repository `repo` (`private` when it is not public) reads a row of this scope, given the
    /// repository the row is on (`None` for an owner's row) and the job's environment. A repository's own rows reach
    /// only its jobs, an environment's only jobs naming it, and an org row as its visibility says: `Private` reaches
    /// private and internal repositories (SHARED, \"Who decrypts\").
    pub fn reaches(
        &self,
        on: Option<graftyard_types::RepoId>,
        repo: graftyard_types::RepoId,
        private: bool,
        env: Option<&EnvName>,
    ) -> bool {
        match self {
            SecretScope::Repo => on == Some(repo),
            SecretScope::Env(e) => on == Some(repo) && env == Some(e),
            SecretScope::Org { visibility, repos } => {
                on.is_none()
                    && match visibility {
                        SecretVisibility::All => true,
                        SecretVisibility::Private => private,
                        SecretVisibility::Selected => repos.contains(&repo),
                    }
            }
        }
    }
}

/// SHARED's `SecretRecord`: the `crypto_box_seal` output the client sent, under the owner's key `key_id`. Rotation
/// replaces `sealed` and `key_id` with a reseal under the new key. `Debug` prints neither the bytes nor their length.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretRecord {
    pub sealed: Box<[u8]>,
    pub key_id: Box<str>,
    pub updated: graftyard_types::Timestamp,
}

impl std::fmt::Debug for SecretRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SecretRecord {{ key_id: {}, updated: {:?} }}", self.key_id, self.updated)
    }
}

/// SHARED's `VariableRecord`. Variables are plain text, as on GitHub.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VariableRecord {
    pub value: Box<str>,
    pub updated: graftyard_types::Timestamp,
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
