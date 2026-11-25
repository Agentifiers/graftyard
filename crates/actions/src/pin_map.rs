//! Vendor-signed action pin maps and org-policy substitution (`design/SHARED.md`, "Toolkit hostname heuristics and SHA
//! pins"; `decisions/DECISIONS.md`, "`pin-map`"). Verification, lookup, and substitution are pure functions of their
//! inputs; hosts supply the signed bytes and the approved and known maps.

use std::collections::BTreeSet;

use ed25519_dalek::{Signature, Signer as _, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

/// A signed mapping from upstream action commits to their Graftyard forks.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PinMap {
    pub version: u32,
    pub pins: Vec<Pin>,
}

/// One upstream commit and the reproducibly built fork commit for the same upstream tag.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pin {
    /// `actions/upload-artifact`
    pub upstream: String,
    /// full 40-hex lower-case commit SHA
    pub upstream_sha: String,
    /// `graftyard-actions/upload-artifact`
    pub fork: String,
    pub fork_sha: String,
    /// the upstream tag the fork was built from, e.g. `v4.6.2`
    pub tag: String,
}

/// SHA-256 of the exact signed document bytes.
pub fn digest(doc: &[u8]) -> graftyard_types::Digest {
    graftyard_types::Digest(Sha256::digest(doc).into())
}

/// Why a signed pin map was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    BadSignature,
    NotApproved,
    Malformed(String),
}

/// Verify and decode the exact signed bytes.
///
/// `release_key` is the install's `Keys.release` key, not `Keys.vendor`, the vendor intent key which self-hosted
/// installs do not have (`decisions/DECISIONS.md`, "`pin-map`"). Signature verification precedes the org's digest
/// approval check, which precedes JSON parsing and content validation.
pub fn verify(
    doc: &[u8],
    sig: &[u8; 64],
    release_key: &[u8; 32],
    approved: graftyard_types::Digest,
) -> Result<PinMap, Refused> {
    let key = VerifyingKey::from_bytes(release_key).map_err(|_| Refused::BadSignature)?;
    key.verify_strict(doc, &Signature::from_bytes(sig)).map_err(|_| Refused::BadSignature)?;
    if digest(doc) != approved {
        return Err(Refused::NotApproved);
    }
    let map: PinMap = serde_json::from_slice(doc).map_err(|e| Refused::Malformed(e.to_string()))?;
    if map.version != 1 {
        return Err(Refused::Malformed(format!("unsupported version {}", map.version)));
    }

    let mut seen = BTreeSet::new();
    for (i, pin) in map.pins.iter().enumerate() {
        if !lower_hex_sha(&pin.upstream_sha) {
            return Err(Refused::Malformed(format!("pin {i}: upstream_sha is not 40 lower-case hex characters")));
        }
        if !lower_hex_sha(&pin.fork_sha) {
            return Err(Refused::Malformed(format!("pin {i}: fork_sha is not 40 lower-case hex characters")));
        }
        let Some(upstream_repo) = pin.upstream.strip_prefix("actions/") else {
            return Err(Refused::Malformed(format!("pin {i}: upstream is not under actions/")));
        };
        let Some(fork_repo) = pin.fork.strip_prefix("graftyard-actions/") else {
            return Err(Refused::Malformed(format!("pin {i}: fork is not under graftyard-actions/")));
        };
        if upstream_repo != fork_repo {
            return Err(Refused::Malformed(format!("pin {i}: fork and upstream repository names differ")));
        }
        if !seen.insert((pin.upstream.to_ascii_lowercase(), pin.upstream_sha.as_str())) {
            return Err(Refused::Malformed(format!("pin {i}: duplicate upstream commit")));
        }
    }
    Ok(map)
}

/// Sign a pin-map document for tests and vendor tooling.
pub fn sign(doc: &[u8], seed: &[u8; 32]) -> [u8; 64] {
    SigningKey::from_bytes(seed).sign(doc).to_bytes()
}

impl PinMap {
    /// Find a pin by case-insensitive GitHub repository name and commit SHA.
    pub fn by_sha(&self, upstream: &str, sha: &str) -> Option<&Pin> {
        self.pins
            .iter()
            .find(|pin| pin.upstream.eq_ignore_ascii_case(upstream) && pin.upstream_sha.eq_ignore_ascii_case(sha))
    }
}

/// Upstream action repositories for which the vendor publishes Graftyard forks.
pub const FORKED: &[&str] = &["upload-artifact", "download-artifact", "upload-pages-artifact", "deploy-pages"];

/// The fork repository for an upstream action, if this action has one.
pub fn fork_of(owner: &str, repo: &str) -> Option<String> {
    (owner.eq_ignore_ascii_case("actions") && FORKED.iter().any(|forked| repo.eq_ignore_ascii_case(forked)))
        .then(|| format!("graftyard-actions/{}", repo.to_ascii_lowercase()))
}

/// The major version of a numeric, `v`-prefixed tag, such as `v4` or `V4.6.2`.
pub fn major(version: &str) -> Option<u32> {
    let rest = version.strip_prefix('v').or_else(|| version.strip_prefix('V'))?;
    let mut parts = rest.split('.');
    let major = parts.next()?;
    fn numeric(part: &str) -> bool {
        !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit())
    }
    if !numeric(major) || !parts.all(numeric) {
        return None;
    }
    major.parse().ok()
}

/// How one `uses:` reference resolves under an org's substitution policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Substitution {
    /// Resolve as written.
    AsWritten,
    /// Resolve `fork@version` instead (tag substitution): the same tag in the fork repository.
    Tag { fork: String },
    /// Resolve `fork@fork_sha` instead (an approved pin).
    Pin { fork: String, fork_sha: String },
    /// An upstream v4+ SHA pin the approved map does not cover, or no map is approved: resolve as written (the
    /// upstream bytes fail in their own step, as on GHES) and annotate with the fork SHA when a known map names it.
    Unapproved { fork: String, fork_sha: Option<String> },
}

/// Decide a tag substitution or approved SHA pin without changing an unrecognised reference.
pub fn substitute(
    owner: &str,
    repo: &str,
    version: &str,
    tags: bool,
    approved: Option<&PinMap>,
    known: Option<&PinMap>,
) -> Substitution {
    let Some(fork) = fork_of(owner, repo) else {
        return Substitution::AsWritten;
    };
    if version.len() != 40 || !version.bytes().all(|b| b.is_ascii_hexdigit()) {
        if tags && major(version).is_some_and(|major| major >= 4) {
            return Substitution::Tag { fork };
        }
        return Substitution::AsWritten;
    }

    let upstream = format!("{owner}/{repo}");
    let approved_pin = approved.and_then(|map| map.by_sha(&upstream, version));
    if let Some(pin) = approved_pin {
        return Substitution::Pin { fork: pin.fork.clone(), fork_sha: pin.fork_sha.clone() };
    }
    let known_pin = known.and_then(|map| map.by_sha(&upstream, version));
    if known_pin.or(approved_pin).and_then(|pin| major(&pin.tag)).is_some_and(|major| major >= 4) {
        return Substitution::Unapproved { fork, fork_sha: known_pin.map(|pin| pin.fork_sha.clone()) };
    }
    Substitution::AsWritten
}

fn lower_hex_sha(sha: &str) -> bool {
    sha.len() == 40 && sha.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    const UPSTREAM_SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const FORK_SHA: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn pin(tag: &str) -> Pin {
        Pin {
            upstream: "actions/upload-artifact".into(),
            upstream_sha: UPSTREAM_SHA.into(),
            fork: "graftyard-actions/upload-artifact".into(),
            fork_sha: FORK_SHA.into(),
            tag: tag.into(),
        }
    }

    fn map(tag: &str) -> PinMap {
        PinMap { version: 1, pins: vec![pin(tag)] }
    }

    fn bytes(doc: &PinMap) -> Vec<u8> {
        serde_json::to_vec(doc).unwrap()
    }

    fn content_refusal(doc: PinMap) -> Refused {
        let bytes = bytes(&doc);
        let seed = [7u8; 32];
        let release_key = SigningKey::from_bytes(&seed).verifying_key().to_bytes();
        let signature = sign(&bytes, &seed);
        verify(&bytes, &signature, &release_key, digest(&bytes)).unwrap_err()
    }

    #[test]
    fn digest_is_sha256_of_the_document_bytes() {
        let expected =
            graftyard_types::Digest::parse_hex("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
                .unwrap();
        assert_eq!(digest(b"abc"), expected);
    }

    #[test]
    fn verify_checks_signature_before_approval() {
        let expected = map("v4.6.2");
        let doc = bytes(&expected);
        let seed_a = [7u8; 32];
        let key_a = SigningKey::from_bytes(&seed_a).verifying_key().to_bytes();
        let signature = sign(&doc, &seed_a);

        assert_eq!(verify(&doc, &signature, &key_a, digest(&doc)), Ok(expected));

        let key_b = SigningKey::from_bytes(&[8u8; 32]).verifying_key().to_bytes();
        assert_eq!(verify(&doc, &signature, &key_b, digest(&doc)), Err(Refused::BadSignature));

        let mut changed = doc.clone();
        changed[0] ^= 1;
        assert_eq!(verify(&changed, &signature, &key_a, digest(&doc)), Err(Refused::BadSignature));
        assert_eq!(verify(&doc, &signature, &key_a, digest(b"another document")), Err(Refused::NotApproved));

        // Both checks would fail, but a signature failure must win.
        assert_eq!(verify(&changed, &signature, &key_a, digest(b"yet another document")), Err(Refused::BadSignature));
    }

    #[test]
    fn verify_refuses_malformed_content_after_a_valid_signature() {
        let mut version_two = map("v4.6.2");
        version_two.version = 2;
        assert!(matches!(content_refusal(version_two), Refused::Malformed(_)));

        let mut short_upstream = map("v4.6.2");
        short_upstream.pins[0].upstream_sha.pop();
        assert!(matches!(content_refusal(short_upstream), Refused::Malformed(_)));

        let mut short_fork = map("v4.6.2");
        short_fork.pins[0].fork_sha.pop();
        assert!(matches!(content_refusal(short_fork), Refused::Malformed(_)));

        let mut wrong_fork = map("v4.6.2");
        wrong_fork.pins[0].fork = "graftyard-actions/download-artifact".into();
        assert!(matches!(content_refusal(wrong_fork), Refused::Malformed(_)));

        let mut duplicate = map("v4.6.2");
        duplicate.pins.push(pin("v4.6.2"));
        assert!(matches!(content_refusal(duplicate), Refused::Malformed(_)));
    }

    #[test]
    fn verify_refuses_wrong_owners_and_uppercase_shas() {
        let mut upstream_owner = map("v4.6.2");
        upstream_owner.pins[0].upstream = "other/upload-artifact".into();
        assert!(matches!(content_refusal(upstream_owner), Refused::Malformed(_)));

        let mut fork_owner = map("v4.6.2");
        fork_owner.pins[0].fork = "actions/upload-artifact".into();
        assert!(matches!(content_refusal(fork_owner), Refused::Malformed(_)));

        let mut uppercase = map("v4.6.2");
        uppercase.pins[0].upstream_sha = UPSTREAM_SHA.to_ascii_uppercase();
        assert!(matches!(content_refusal(uppercase), Refused::Malformed(_)));
    }

    #[test]
    fn by_sha_matches_names_and_shas_case_insensitively() {
        let map = map("v4.6.2");
        assert_eq!(map.by_sha("Actions/Upload-Artifact", &UPSTREAM_SHA.to_ascii_uppercase()), map.pins.first());
        assert_eq!(map.by_sha("actions/download-artifact", UPSTREAM_SHA), None);
    }

    #[test]
    fn fork_of_matches_only_forked_actions_repositories() {
        assert_eq!(fork_of("Actions", "Upload-Artifact").as_deref(), Some("graftyard-actions/upload-artifact"));
        assert_eq!(fork_of("actions", "checkout"), None);
        assert_eq!(fork_of("other", "upload-artifact"), None);
    }

    #[test]
    fn substitute_applies_tag_policy_only_to_v4_and_newer_forks() {
        assert_eq!(
            substitute("actions", "upload-artifact", "v4", true, None, None),
            Substitution::Tag { fork: "graftyard-actions/upload-artifact".into() }
        );
        assert_eq!(substitute("actions", "upload-artifact", "v4", false, None, None), Substitution::AsWritten);
        assert_eq!(substitute("actions", "upload-artifact", "v3.2.2", true, None, None), Substitution::AsWritten);
        assert_eq!(substitute("actions", "checkout", "v4", true, None, None), Substitution::AsWritten);
        assert_eq!(substitute("actions", "upload-artifact", "main", true, None, None), Substitution::AsWritten);
    }

    #[test]
    fn substitute_uses_an_approved_sha_pin() {
        let approved = map("v4.6.2");
        assert_eq!(
            substitute("actions", "upload-artifact", UPSTREAM_SHA, false, Some(&approved), None),
            Substitution::Pin { fork: "graftyard-actions/upload-artifact".into(), fork_sha: FORK_SHA.into() }
        );
    }

    #[test]
    fn substitute_annotates_only_known_unapproved_v4_sha_pins() {
        let known = map("v4.6.2");
        let empty = PinMap { version: 1, pins: vec![] };
        assert_eq!(
            substitute("actions", "upload-artifact", UPSTREAM_SHA, false, Some(&empty), Some(&known)),
            Substitution::Unapproved {
                fork: "graftyard-actions/upload-artifact".into(),
                fork_sha: Some(FORK_SHA.into()),
            }
        );
        assert_eq!(
            substitute("actions", "upload-artifact", UPSTREAM_SHA, false, None, Some(&known)),
            Substitution::Unapproved {
                fork: "graftyard-actions/upload-artifact".into(),
                fork_sha: Some(FORK_SHA.into()),
            }
        );
        assert_eq!(substitute("actions", "upload-artifact", UPSTREAM_SHA, false, None, None), Substitution::AsWritten);

        let old = map("v3.2.2");
        assert_eq!(
            substitute("actions", "upload-artifact", UPSTREAM_SHA, false, None, Some(&old)),
            Substitution::AsWritten
        );
    }

    #[test]
    fn major_accepts_only_numeric_v_prefixed_tags() {
        assert_eq!(major("v4"), Some(4));
        assert_eq!(major("v4.6.2"), Some(4));
        assert_eq!(major("V10.1"), Some(10));
        assert_eq!(major("main"), None);
        assert_eq!(major("4"), None);
        assert_eq!(major(UPSTREAM_SHA), None);
        assert_eq!(major("v4."), None);
        assert_eq!(major("v4-beta"), None);
    }
}
