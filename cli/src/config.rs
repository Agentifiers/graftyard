//! `install.toml` and the key files the operator passes (`design/DESIGN.md`, "Install operations", Genesis).

use std::fs;
use std::path::Path;

use base64::Engine as _;
use graftyard_release::{Anchor, InstallId, KeyId};
use graftyard_types::{PlacementRecord, Rtt, Site, SiteId, SiteTable, Timestamp, Ulid};
use serde::{Deserialize, Serialize};

pub type Res<T> = Result<T, String>;

/// What `graftyard init` writes and every later command reads. `install` and `customer_key` are the trust anchor and
/// never change.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InstallToml {
    /// The install id, as `GRAFTYARD_INSTALL` holds it.
    pub install: String,
    /// The Cloudflare account the install's scripts live in.
    pub account: String,
    pub zone: String,
    /// The customer public key, as `GRAFTYARD_CUSTOMER_KEY` holds it: 64 lower-case hex characters.
    pub customer_key: String,
    /// The signed `evaluation` flag the genesis will carry. Only an evaluation install may place `Solo`.
    #[serde(default)]
    pub evaluation: bool,
    /// The digest of the genesis the edge committed, once `graftyard install --genesis` saw 201.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub genesis: Option<String>,
    #[serde(default)]
    pub sites: Vec<Site>,
    /// The latency check's readings, one per measured (from, peer) pair, from `sites add`.
    #[serde(default)]
    pub rtt: Vec<RttRow>,
    /// The install account's record as `placement set default` last emitted it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_placement: Option<PlacementRecord>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RttRow {
    pub from: SiteId,
    pub peer: SiteId,
    pub median_ms: u16,
    pub at: Timestamp,
}

impl InstallToml {
    pub fn load(path: &Path) -> Res<InstallToml> {
        let text =
            fs::read_to_string(path).map_err(|e| format!("{}: {e} (run `graftyard init` first)", path.display()))?;
        let t: InstallToml = toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        t.anchor().map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(t)
    }

    pub fn save(&self, path: &Path) -> Res<()> {
        let text = toml::to_string(self).map_err(|e| e.to_string())?;
        let tmp = path.with_extension("toml.tmp");
        fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
        fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn anchor(&self) -> Res<Anchor> {
        Anchor::from_vars(&self.install, &self.customer_key).map_err(|e| e.to_string())
    }

    pub fn table(&self) -> SiteTable {
        SiteTable(self.sites.clone())
    }

    pub fn site_by_script(&self, script: &str) -> Option<&Site> {
        self.sites.iter().find(|s| s.script == script)
    }

    /// The rows `Placement::new` reads, as `(measuring site, Rtt)`.
    pub fn rtt_rows(&self) -> Vec<(SiteId, Rtt)> {
        self.rtt.iter().map(|r| (r.from, Rtt { peer: r.peer, median_ms: r.median_ms, at: r.at })).collect()
    }

    /// Keeps one row per (from, peer), the newest.
    pub fn put_rtt(&mut self, row: RttRow) {
        self.rtt.retain(|r| !(r.from == row.from && r.peer == row.peer));
        self.rtt.push(row);
        self.rtt.sort_by_key(|r| (r.from, r.peer));
    }

    /// The install separates accounts when any site lives outside the install's own account.
    pub fn separate_accounts(&self) -> bool {
        self.sites.iter().any(|s| s.account != self.account)
    }
}

pub fn now() -> Timestamp {
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("clock after 1970").as_millis();
    Timestamp(ms as u64)
}

pub fn random<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    getrandom::fill(&mut b).expect("the OS random source");
    b
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn mint_install() -> InstallId {
    InstallId(Ulid::new(now(), random()))
}

// ---- Key files ----

/// DER prefix of an Ed25519 `SubjectPublicKeyInfo` (RFC 8410), followed by the 32-byte key.
const SPKI_PREFIX: [u8; 12] = [0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00];
/// DER prefix of an Ed25519 PKCS#8 v1 `PrivateKeyInfo` (RFC 8410), followed by the 32-byte seed.
const PKCS8_PREFIX: [u8; 16] =
    [0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20];

fn pem(text: &str, label: &str) -> Option<Vec<u8>> {
    let begin = format!("-----BEGIN {label}-----");
    let end = format!("-----END {label}-----");
    let body = text.split_once(&begin)?.1.split_once(&end)?.0;
    let b64: String = body.chars().filter(|c| !c.is_whitespace()).collect();
    base64::engine::general_purpose::STANDARD.decode(b64).ok()
}

fn hex32(text: &str) -> Option<[u8; 32]> {
    let t = text.trim();
    KeyId::parse_hex(&t.to_ascii_lowercase()).map(|k| k.0)
}

/// A public key file: 64 hex characters, or the PEM `PUBLIC KEY` that `openssl pkey -pubout` writes for Ed25519.
pub fn parse_public_key(text: &str) -> Res<KeyId> {
    if let Some(k) = hex32(text) {
        return Ok(KeyId(k));
    }
    if let Some(der) = pem(text, "PUBLIC KEY")
        && der.len() == 44
        && der[..12] == SPKI_PREFIX
    {
        return Ok(KeyId(der[12..].try_into().expect("32 bytes")));
    }
    Err("not an Ed25519 public key: expected 64 hex characters or an Ed25519 PEM PUBLIC KEY".into())
}

/// A private key file: a 64-hex-character seed, or the PEM `PRIVATE KEY` that `openssl genpkey -algorithm ed25519`
/// writes (PKCS#8 v1, or v2 with the public key attached).
pub fn parse_private_key(text: &str) -> Res<[u8; 32]> {
    if let Some(k) = hex32(text) {
        return Ok(k);
    }
    if let Some(der) = pem(text, "PRIVATE KEY") {
        if der.len() == 48 && der[..16] == PKCS8_PREFIX {
            return Ok(der[16..].try_into().expect("32 bytes"));
        }
        // v2 (OneAsymmetricKey): version 1, then the same algorithm and seed, then the public key.
        let v2 = [0x02, 0x01, 0x01, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20];
        if der.len() > 2 + v2.len() + 32 && der[0] == 0x30 && der[2..2 + v2.len()] == v2 {
            let seed: [u8; 32] = der[2 + v2.len()..2 + v2.len() + 32].try_into().expect("32 bytes");
            return Ok(seed);
        }
    }
    Err("not an Ed25519 private key: expected a 64-hex-character seed or an Ed25519 PEM PRIVATE KEY".into())
}

pub fn read_public_key(path: &Path) -> Res<KeyId> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse_public_key(&text).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn read_private_key(path: &Path) -> Res<[u8; 32]> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse_private_key(&text).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use graftyard_release::public_key;
    use graftyard_types::{Region, SiteLink};

    use super::*;

    #[test]
    fn install_toml_round_trips_with_sites_and_rows() {
        let t = InstallToml {
            install: mint_install().to_string(),
            account: "be2a".into(),
            zone: "git.acme.example".into(),
            customer_key: public_key(&[7; 32]).hex(),
            evaluation: false,
            genesis: None,
            sites: vec![Site {
                id: SiteId(0),
                script: "graft-site-a".into(),
                account: "be2a".into(),
                region: Region::Enam,
                jurisdiction: Some(graftyard_types::Jurisdiction::Us),
                bucket: "graft-site-a-packs".into(),
                release: None,
                link: SiteLink::Binding { member_class: "Member".into() },
                spare: false,
            }],
            rtt: vec![RttRow { from: SiteId(0), peer: SiteId(1), median_ms: 98, at: Timestamp(3) }],
            default_placement: None,
        };
        let text = toml::to_string(&t).unwrap();
        assert_eq!(toml::from_str::<InstallToml>(&text).unwrap(), t, "{text}");
    }

    #[test]
    fn key_files_parse_in_hex_and_openssl_pem() {
        // RFC 8032 section 7.1 test 1, as `openssl pkey` writes its private key and `openssl pkey -pubout` its public.
        let private = "-----BEGIN PRIVATE KEY-----\nMC4CAQAwBQYDK2VwBCIEIJ1hsZ3v/VpguoRK9JLsLMREScVpezJpGXA7rAMcrn9g\n-----END PRIVATE KEY-----\n";
        let public = "-----BEGIN PUBLIC KEY-----\nMCowBQYDK2VwAyEA11qYAYKxCrfVS/7TyWQHOg7hcvPapiMlrwIaaPcHURo=\n-----END PUBLIC KEY-----\n";
        let seed = parse_private_key(private).unwrap();
        assert_eq!(KeyId(seed).hex(), "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60");
        assert_eq!(public_key(&seed), parse_public_key(public).unwrap());
        assert_eq!(public_key(&seed).hex(), "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a");
        assert_eq!(parse_private_key(&format!("{}\n", KeyId(seed).hex())).unwrap(), seed);
        assert!(parse_public_key("ssh-rsa AAAA").is_err());
        assert!(parse_private_key(public).is_err());
    }
}
