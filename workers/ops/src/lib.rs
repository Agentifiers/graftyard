//! `graft-ops`'s installation-token minting (`build/BUILD-PLAN.md`, M3-WP4; `design/DESIGN.md`, Stack, Secrets Store:
//! "the key that wraps the installation-token signing key, which puts it on the minting path"). The edge reaches it
//! only through a `fetch()`-based service binding; the script has no public route.
//!
//! `POST /mint` takes a `Grant` the edge built with `access_token_grant` and answers `{"token", "expires_at", "jti",
//! "kid"}`. The signing key is the `MintWrapKey` secret unwrapped (`MintKeySource`), held by the isolate's `Minter`
//! for at most `SIGNING_KEY_CACHE`, so a rotated secret is read within that long without a new script version.
//!
//! The executor of signed intents runs on the Cron Trigger (`exec`, M5-WP3). The API-calling crons and `graftyard
//! mint-key rotate` are not built.

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

/// The `TokenShard(0)` command a `MintWrapKey` `Rotate` step commits as the operator (M5-WP6;
/// `decisions/DECISIONS.md`, "`rotate-probe`"): `Publish` the new slot's key with no `retired`, or `Retire` the old
/// slot's key one `SIGNING_KEY_CACHE`, one `JOB_TOKEN_MAX` and one `CLOCK_SKEW` after `switched`, as `rotation_step`
/// says. A rerun after a kill commits the same row again, which `decide` takes, since the key is unchanged. `None`
/// for a step that commits nothing. `old_text` is read only at `Retire`; an old slot holding the new slot's kid is an
/// error, since retiring it would retire the key that signs.
pub fn rotate_command(
    step: graftyard_release::RotateStep,
    new_text: &str,
    old_text: &str,
    switched: graftyard_types::Timestamp,
) -> Result<Option<graftyard_identity::TokensCmd>, KeyError> {
    use graftyard_identity::stateless::{KeyEpoch, RotationStep, rotation_step};
    use graftyard_identity::{TokensCmd, TokensOp};
    use graftyard_types::CredentialShard;
    let (kid, new) = unwrap(new_text)?;
    let op = match step {
        graftyard_release::RotateStep::Publish => TokensOp::PutVerifyingKey { kid, key: new.public(), retired: None },
        graftyard_release::RotateStep::Retire => {
            let (old_kid, old) = unwrap(old_text)?;
            if old_kid == kid {
                return Err(bad("the old slot's key has the new slot's kid"));
            }
            let epoch = |kid, switched| KeyEpoch { kid, published: switched, switched: Some(switched) };
            let RotationStep::Retire { kid, at } =
                rotation_step(&epoch(old_kid, switched), &epoch(kid, switched), switched)
            else {
                unreachable!("a switched key retires")
            };
            TokensOp::PutVerifyingKey { kid, key: old.public(), retired: Some(at) }
        }
        _ => return Ok(None),
    };
    Ok(Some(TokensCmd { cred: CredentialShard::NONE, op }))
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

// ---- Actions secrets (`design/SHARED.md`, "Actions", Secrets and variables; BF-59)

/// What `/actions/open` opens for one job: each secret's name, its `crypto_box_seal` bytes, and the `key_id` it was
/// sealed to, with the owner's keys the records name. Only `graft-ops` holds `SecretsWrapKey`, so only it opens a
/// `SealKey`'s private half, and the plaintexts leave only in the answer to the job's own `acquirejob`.
#[derive(Clone, Serialize, Deserialize)]
pub struct OpenRequest {
    pub keys: Vec<graftyard_identity::sealed::SealKey>,
    pub secrets: Vec<(String, Box<[u8]>, Box<str>)>,
}

/// A secret's name and its `crypto_box_seal` bytes.
pub type Sealed = (String, Box<[u8]>);

/// One owner's private key, unwrapped. Never printed.
struct OpenKey(crypto_box::SecretKey);

fn open_key(src: &WrapKeySource, k: &graftyard_identity::sealed::SealKey) -> Result<OpenKey, KeyError> {
    let raw = src.unwrap(&k.private)?;
    let raw: [u8; 32] = raw.0.as_ref().try_into().map_err(|_| bad("seal key length"))?;
    let sk = crypto_box::SecretKey::from_bytes(raw);
    if sk.public_key().as_bytes() != &k.public {
        return Err(bad("the seal key's private half does not match its public half"));
    }
    Ok(OpenKey(sk))
}

/// A new `SealKey` for an owner from 32 random bytes: its private half wrapped under `SecretsWrapKey`'s current slot,
/// and a decimal `key_id` from the public half, as GitHub's are decimal [inference from `gh secret set`'s `key_id`].
pub fn mint_seal_key(
    src: &WrapKeySource,
    seed: [u8; 32],
    nonce: [u8; 12],
    now: graftyard_types::Timestamp,
) -> Result<graftyard_identity::sealed::SealKey, KeyError> {
    let sk = crypto_box::SecretKey::from_bytes(seed);
    let public = *sk.public_key().as_bytes();
    let private = src.wrap(&sk.to_bytes(), nonce)?;
    let id = u64::from_be_bytes(public[..8].try_into().expect("8 bytes")) >> 1;
    Ok(graftyard_identity::sealed::SealKey {
        key_id: id.to_string().into(),
        public,
        private,
        created: now,
        retired: None,
    })
}

/// The plaintexts of `r.secrets`, by name. A record whose key is missing or does not open it is left out and named in
/// the second list, so a job runs without it rather than with a wrong value.
pub fn open_secrets(src: &WrapKeySource, r: &OpenRequest) -> (Vec<(String, String)>, Vec<String>) {
    let keys: Vec<_> = r.keys.iter().map(|k| (k.key_id.clone(), open_key(src, k))).collect();
    let (mut out, mut failed) = (Vec::new(), Vec::new());
    for (name, sealed, key_id) in &r.secrets {
        let opened = keys
            .iter()
            .find(|(id, _)| id == key_id)
            .and_then(|(_, k)| k.as_ref().ok())
            .and_then(|k| k.0.unseal(sealed).ok())
            .and_then(|p| String::from_utf8(p).ok());
        match opened {
            Some(p) => out.push((name.clone(), p)),
            None => failed.push(name.clone()),
        }
    }
    (out, failed)
}

/// The rotation's reseal: each record opened with its old key and sealed to `to`, inside this crate, so no plaintext
/// leaves. `rng` draws each seal's ephemeral key. A record that does not open is returned unchanged and named.
pub fn reseal(
    src: &WrapKeySource,
    r: &OpenRequest,
    to: &graftyard_identity::sealed::SealKey,
    rng: &mut impl crypto_box::aead::rand_core::CryptoRngCore,
) -> (Vec<Sealed>, Vec<String>) {
    let target = crypto_box::PublicKey::from(to.public);
    let keys: Vec<_> = r.keys.iter().map(|k| (k.key_id.clone(), open_key(src, k))).collect();
    let (mut out, mut failed) = (Vec::new(), Vec::new());
    for (name, sealed, key_id) in &r.secrets {
        let plain = keys
            .iter()
            .find(|(id, _)| id == key_id)
            .and_then(|(_, k)| k.as_ref().ok())
            .and_then(|k| k.0.unseal(sealed).ok());
        match plain.and_then(|p| target.seal(rng, &p).ok()) {
            Some(s) => out.push((name.clone(), s.into())),
            None => failed.push(name.clone()),
        }
    }
    (out, failed)
}

/// A `Wrapped` value under the current slot, for the wrapping key's rotation: opened and wrapped again inside this
/// crate, so its plaintext never leaves. A value already under the current slot is returned as it is.
pub fn rewrap(src: &WrapKeySource, w: &Wrapped, nonce: [u8; 12]) -> Result<Wrapped, KeyError> {
    if w.slot == src.current {
        return Ok(w.clone());
    }
    src.wrap(&src.unwrap(w)?.0, nonce)
}

// ---- `Rotate` probes (`decisions/DECISIONS.md`, "`rotate-probe`"; OPEN-QUESTIONS R45)

/// The Secrets Store bindings of `SecretsWrapKey`'s slots 0 and 1. Both stay bound, so a value wrapped under either
/// unwraps whichever slot new values take.
pub const WRAP_KEY_BINDINGS: [&str; 2] = ["SECRETS_WRAP_KEY_A", "SECRETS_WRAP_KEY_B"];

/// The `SecretsWrapKey` `Rotate`'s probe: the new slot's text is a `WrapKeySlot` naming that slot, since a value
/// wrapped under it carries the number, with a 32-byte key. The error never holds the key.
pub fn wrap_slot_ok(text: &str, slot: u8) -> Result<(), &'static str> {
    let s: WrapKeySlot = serde_json::from_str(text).map_err(|_| "not a SecretsWrapKey slot")?;
    if s.slot != slot {
        return Err("its text names another slot number");
    }
    unhex(&s.key).filter(|k| k.len() == 32).ok_or("its key is not 32 bytes of hex")?;
    Ok(())
}

/// The App-key `Rotate`s' probe: the App's record in the install account's `Authz` fold already lists the new slot's
/// value, so the switch moves no holder to a key or client secret the App refuses. For `McpAppKey` and
/// `InsightsAppKey`, `value` is a PEM RSA private key (PKCS#1 or PKCS#8) whose public half, as DER
/// `SubjectPublicKeyInfo`, is one of `rec.keys`; for `McpClientSecret`, its hash is one of `rec.client_secrets`. The
/// error says which check failed and never holds the value.
pub fn app_lists(
    secret: graftyard_release::SecretName,
    value: &str,
    rec: &graftyard_identity::AppRecord,
) -> Result<(), &'static str> {
    use graftyard_release::SecretName as S;
    match secret {
        S::McpClientSecret => graftyard_identity::appauth::client_secret_ok(&rec.client_secrets, value)
            .then_some(())
            .ok_or("the App lists no client secret with the new slot's hash"),
        S::McpAppKey | S::InsightsAppKey => {
            let public = rsa_public_der(value)?;
            rec.keys
                .iter()
                .any(|k| k.public.0[..] == public[..])
                .then_some(())
                .ok_or("the App lists no key with the new slot's public half")
        }
        _ => Err("not an App's key or client secret"),
    }
}

/// A PEM RSA private key's public half as DER `SubjectPublicKeyInfo`. The PEM armour is stripped by hand, as
/// `workers/runners` does, so `rsa` needs no `pem` feature.
fn rsa_public_der(pem: &str) -> Result<Vec<u8>, &'static str> {
    use base64::Engine as _;
    use rsa::pkcs1::DecodeRsaPrivateKey as _;
    use rsa::pkcs8::{DecodePrivateKey as _, EncodePublicKey as _};
    let body: String = pem.lines().map(str::trim).filter(|l| !l.starts_with("-----")).collect();
    let der = base64::engine::general_purpose::STANDARD.decode(body).map_err(|_| "the new slot's key is not PEM")?;
    let key = rsa::RsaPrivateKey::from_pkcs1_der(&der)
        .or_else(|_| rsa::RsaPrivateKey::from_pkcs8_der(&der))
        .map_err(|_| "the new slot's key is not an RSA private key in PKCS#1 or PKCS#8")?;
    let spki = key.to_public_key().to_public_key_der().map_err(|_| "the new slot's public half does not encode")?;
    Ok(spki.as_bytes().to_vec())
}

/// One public half a site Registry's `GET /ssh_slots` lists for a slot.
#[derive(Clone, Debug, Deserialize)]
pub struct SshHostPublic {
    #[serde(rename = "type")]
    pub kind: String,
    pub key: String,
    pub fingerprint: String,
}

/// The host-key types every `SshHostKeys` slot holds, one key each.
pub const SSH_HOST_KEY_TYPES: [&str; 3] = ["ssh-ed25519", "ecdsa-sha2-nistp256", "ssh-rsa"];

/// The `SshHostKeys` `Rotate`'s probe over each non-spare site's new slot, by site id: exactly one key of each
/// `SSH_HOST_KEY_TYPES` and no other, and the same fingerprints on every site, so a client that learned the slot from
/// one site trusts every other. Answers the fingerprints in `SSH_HOST_KEY_TYPES` order.
pub fn ssh_slot_ready(sites: &[(u8, Vec<SshHostPublic>)]) -> Result<Vec<String>, String> {
    let mut first: Option<(u8, Vec<String>)> = None;
    for (site, keys) in sites {
        let mut fps = Vec::new();
        for t in SSH_HOST_KEY_TYPES {
            match keys.iter().filter(|k| k.kind == t).count() {
                1 => fps.extend(keys.iter().filter(|k| k.kind == t).map(|k| k.fingerprint.clone())),
                n => return Err(format!("site {site} holds {n} {t} keys in the new slot, not 1")),
            }
        }
        if keys.len() != SSH_HOST_KEY_TYPES.len() {
            return Err(format!("site {site} holds {} keys in the new slot, not 3", keys.len()));
        }
        match &first {
            None => first = Some((*site, fps)),
            Some((s0, f0)) if *f0 != fps => {
                return Err(format!("site {site}'s new-slot fingerprints {fps:?} differ from site {s0}'s {f0:?}"));
            }
            Some(_) => {}
        }
    }
    first.map(|(_, f)| f).ok_or_else(|| "no non-spare site to probe".into())
}

#[cfg(target_arch = "wasm32")]
mod exec;
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

    /// A `crypto_box_seal` from PyNaCl (libsodium), as `gh secret set` seals with Go's `box.SealAnonymous`, opens
    /// under the minted key; it does not open under another owner's key or a tampered one; a rotation reseals it to a
    /// new key that opens it and the old one no longer does. The PyNaCl ciphertext was printed by
    /// `uv run --with pynacl` with `SealedBox(PrivateKey(bytes(range(1, 33))).public_key).encrypt(b"s3cr3t value")`.
    #[test]
    fn a_libsodium_sealed_box_opens_only_under_its_key_and_reseals_to_a_new_one() {
        let slot = serde_json::to_string(&WrapKeySlot { slot: 0, key: hex(&[4; 32]) }).unwrap();
        let src = WrapKeySource::parse(0, &[&slot]).unwrap();
        let seed: [u8; 32] = core::array::from_fn(|i| i as u8 + 1);
        let k = mint_seal_key(&src, seed, [3; 12], graftyard_types::Timestamp(1)).unwrap();
        assert_eq!(hex(&k.public), "07a37cbc142093c8b755dc1b10e86cb426374ad16aa853ed0bdfc0b2b86d1c7c");
        assert!(!k.private.ciphertext.windows(32).any(|w| w == seed), "the private half is stored only wrapped");
        let sealed: Box<[u8]> = unhex(
            "c9b9da4b5b6e9c59f3b4fe3a4f75509ff8f1b49743d4746b56f60021a464b51bc93fdc6b0d39023bd733da36e7c049b39d512eabbfa795f9bd58c22e",
        )
        .unwrap()
        .into();
        let req = |keys: Vec<graftyard_identity::sealed::SealKey>, sealed: Box<[u8]>, id: &str| OpenRequest {
            keys,
            secrets: vec![("TOKEN".into(), sealed, id.into())],
        };
        let (open, failed) = open_secrets(&src, &req(vec![k.clone()], sealed.clone(), &k.key_id));
        assert_eq!((open, failed), (vec![("TOKEN".into(), "s3cr3t value".into())], Vec::<String>::new()));
        let other = mint_seal_key(&src, [9; 32], [3; 12], graftyard_types::Timestamp(1)).unwrap();
        let wrong = OpenRequest { keys: vec![other.clone()], ..req(vec![], sealed.clone(), &other.key_id) };
        assert_eq!(open_secrets(&src, &wrong), (vec![], vec!["TOKEN".to_owned()]));
        let mut t = sealed.to_vec();
        t[40] ^= 1;
        assert_eq!(open_secrets(&src, &req(vec![k.clone()], t.into(), &k.key_id)).1, vec!["TOKEN".to_owned()]);
        // Rotation: resealed to `other`, the record opens under it and not under the old key.
        struct Fixed(u8);
        impl crypto_box::aead::rand_core::RngCore for Fixed {
            fn next_u32(&mut self) -> u32 {
                0
            }
            fn next_u64(&mut self) -> u64 {
                0
            }
            fn fill_bytes(&mut self, d: &mut [u8]) {
                d.fill(self.0);
            }
            fn try_fill_bytes(&mut self, d: &mut [u8]) -> Result<(), crypto_box::aead::rand_core::Error> {
                d.fill(self.0);
                Ok(())
            }
        }
        impl crypto_box::aead::rand_core::CryptoRng for Fixed {}
        let (re, failed) = reseal(&src, &req(vec![k.clone()], sealed, &k.key_id), &other, &mut Fixed(5));
        assert!(failed.is_empty());
        let both = vec![k.clone(), other.clone()];
        assert_eq!(open_secrets(&src, &req(both.clone(), re[0].1.clone(), &other.key_id)).0[0].1, "s3cr3t value");
        assert_eq!(open_secrets(&src, &req(both, re[0].1.clone(), &k.key_id)).1, vec!["TOKEN".to_owned()]);
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

    /// M5-WP6's `MintWrapKey` rows: `Publish` names the new slot's kid and key with no `retired`; `Retire` names the
    /// old slot's, retired a cache, the longest token lifetime and a skew after the switch, as `mint-key step` says.
    #[test]
    fn a_mint_wrap_key_rotate_publishes_the_new_key_and_retires_the_old_after_the_longest_token() {
        use graftyard_identity::stateless::{CLOCK_SKEW, INSTALLATION_TOKEN_TTL, JOB_TOKEN_MAX, SIGNING_KEY_CACHE};
        use graftyard_identity::{TokensOp, stateless::SigningSecret};
        use graftyard_release::RotateStep;
        use graftyard_types::Timestamp;
        let text = |kid: u32, seed: u8| {
            serde_json::to_string(&wrap([seed; 32], Kid(kid), [1; 12], &SigningSecret([seed; 32]))).unwrap()
        };
        let (old, new) = (text(3, 7), text(4, 8));
        let switched = Timestamp(1_800_000_000_000);
        let publish = rotate_command(RotateStep::Publish, &new, &old, switched).unwrap().unwrap();
        assert!(publish.cred.is_operator());
        assert_eq!(
            publish.op,
            TokensOp::PutVerifyingKey { kid: Kid(4), key: SigningSecret([8; 32]).public(), retired: None }
        );
        let retire = rotate_command(RotateStep::Retire, &new, &old, switched).unwrap().unwrap();
        let after = (SIGNING_KEY_CACHE + JOB_TOKEN_MAX + CLOCK_SKEW).as_millis() as u64;
        assert_eq!(
            retire.op,
            TokensOp::PutVerifyingKey {
                kid: Kid(3),
                key: SigningSecret([7; 32]).public(),
                retired: Some(Timestamp(switched.0 + after))
            }
        );
        // The executor waits one installation-token lifetime between the two, the same constant `rotation_step` uses.
        assert_eq!(graftyard_release::ops::ROTATE_PUBLISHED_FOR, INSTALLATION_TOKEN_TTL.as_millis() as u64);
        // An old slot holding the new kid would retire the key that signs: refused. A probe step commits nothing.
        assert!(rotate_command(RotateStep::Retire, &new, &text(4, 9), switched).is_err());
        assert!(rotate_command(RotateStep::Probe, &new, &old, switched).unwrap().is_none());
        assert!(rotate_command(RotateStep::Publish, "not a key", &old, switched).is_err());
    }

    /// `SecretsWrapKey`'s probe: the slot's text must name the Rotate's slot and hold a 32-byte key.
    #[test]
    fn a_wrap_key_slot_passes_only_naming_its_slot_with_a_32_byte_key() {
        let text = |slot: u8, key: &str| serde_json::to_string(&WrapKeySlot { slot, key: key.into() }).unwrap();
        assert_eq!(wrap_slot_ok(&text(1, &hex(&[4; 32])), 1), Ok(()));
        assert!(wrap_slot_ok(&text(0, &hex(&[4; 32])), 1).is_err(), "names the other slot");
        assert!(wrap_slot_ok(&text(1, &hex(&[4; 31])), 1).is_err(), "31 bytes");
        assert!(wrap_slot_ok(&text(1, "zz"), 1).is_err(), "not hex");
        assert!(wrap_slot_ok("not json", 1).is_err());
    }

    fn pem(label: &str, der: &[u8]) -> String {
        use base64::Engine as _;
        let b = base64::engine::general_purpose::STANDARD.encode(der);
        let lines: Vec<&str> = b.as_bytes().chunks(64).map(|c| std::str::from_utf8(c).unwrap()).collect();
        format!("-----BEGIN {label}-----\n{}\n-----END {label}-----\n", lines.join("\n"))
    }

    fn app(keys: &[&rsa::RsaPrivateKey], secrets: &[&str]) -> graftyard_identity::AppRecord {
        use graftyard_identity::{AppId, AppKey, AppRecord, ClientId, RsaPublic, Slug};
        use graftyard_types::{DbId, Timestamp};
        use rsa::pkcs8::EncodePublicKey as _;
        AppRecord {
            id: AppId(DbId(7)),
            slug: Slug::parse("graftyard-mcp").unwrap(),
            name: "Graftyard MCP".into(),
            client: ClientId::parse("Iv1.0123456789abcdef").unwrap(),
            keys: keys
                .iter()
                .map(|k| AppKey {
                    id: DbId(1),
                    public: RsaPublic(k.to_public_key().to_public_key_der().unwrap().as_bytes().into()),
                    created: Timestamp(0),
                })
                .collect::<Vec<_>>()
                .into(),
            permissions: Default::default(),
            events: Vec::new().into(),
            webhook: None,
            webhook_secret: None,
            client_secrets: secrets
                .iter()
                .map(|s| graftyard_identity::appauth::secret_hash(s))
                .collect::<Vec<_>>()
                .into(),
            callbacks: Vec::new().into(),
            public: false,
        }
    }

    /// The App-key probes: a key passes, as PKCS#8 or PKCS#1 PEM, only when the App lists its public half; a client
    /// secret only when the App lists its hash; a PEM that does not parse fails, and no error holds the value.
    #[test]
    fn an_app_key_or_client_secret_passes_only_when_the_app_lists_it() {
        use graftyard_release::SecretName as S;
        use rand::SeedableRng as _;
        use rsa::pkcs1::EncodeRsaPrivateKey as _;
        use rsa::pkcs8::EncodePrivateKey as _;
        let mut rng = rand::rngs::StdRng::seed_from_u64(45);
        let listed = rsa::RsaPrivateKey::new(&mut rng, 1024).unwrap();
        let other = rsa::RsaPrivateKey::new(&mut rng, 1024).unwrap();
        let rec = app(&[&other, &listed], &["old secret", "new secret"]);
        let pkcs8 = |k: &rsa::RsaPrivateKey| pem("PRIVATE KEY", k.to_pkcs8_der().unwrap().as_bytes());
        let pkcs1 = pem("RSA PRIVATE KEY", listed.to_pkcs1_der().unwrap().as_bytes());
        for s in [S::McpAppKey, S::InsightsAppKey] {
            assert_eq!(app_lists(s, &pkcs8(&listed), &rec), Ok(()));
            assert_eq!(app_lists(s, &pkcs1, &rec), Ok(()));
        }
        let unlisted = pkcs8(&rsa::RsaPrivateKey::new(&mut rng, 1024).unwrap());
        assert_eq!(
            app_lists(S::McpAppKey, &unlisted, &rec),
            Err("the App lists no key with the new slot's public half")
        );
        assert_eq!(app_lists(S::McpClientSecret, "new secret", &rec), Ok(()));
        assert_eq!(
            app_lists(S::McpClientSecret, "another secret", &rec),
            Err("the App lists no client secret with the new slot's hash")
        );
        let broken = pkcs8(&listed).replacen('M', "!", 1);
        assert_eq!(app_lists(S::McpAppKey, &broken, &rec), Err("the new slot's key is not PEM"));
        let not_rsa = pem("PRIVATE KEY", b"not a key at all");
        assert!(app_lists(S::InsightsAppKey, &not_rsa, &rec).unwrap_err().contains("not an RSA private key"));
        assert!(app_lists(S::DeployToken, "new secret", &rec).is_err());
    }

    fn ssh(kind: &str, fp: &str) -> SshHostPublic {
        SshHostPublic { kind: kind.into(), key: format!("AAAA{fp}"), fingerprint: format!("SHA256:{fp}") }
    }

    fn slot(rsa_fp: &str) -> Vec<SshHostPublic> {
        vec![ssh("ssh-ed25519", "ed"), ssh("ecdsa-sha2-nistp256", "ec"), ssh("ssh-rsa", rsa_fp)]
    }

    /// `SshHostKeys`' probe: three sites holding the same three keys pass and name the fingerprints; a site missing
    /// its RSA key, a site with an extra key, or two sites with different fingerprints fail, naming the site.
    #[test]
    fn an_ssh_slot_passes_only_with_the_same_three_keys_on_every_site() {
        let good: Vec<_> = (0..3).map(|s| (s, slot("rsa"))).collect();
        assert_eq!(ssh_slot_ready(&good).unwrap(), vec!["SHA256:ed", "SHA256:ec", "SHA256:rsa"]);
        let mut missing = good.clone();
        missing[1].1.pop();
        assert_eq!(ssh_slot_ready(&missing).unwrap_err(), "site 1 holds 0 ssh-rsa keys in the new slot, not 1");
        let mut extra = good.clone();
        extra[2].1.push(ssh("ssh-dss", "dss"));
        assert_eq!(ssh_slot_ready(&extra).unwrap_err(), "site 2 holds 4 keys in the new slot, not 3");
        let mut differ = good.clone();
        differ[2].1 = slot("rsa2");
        assert!(ssh_slot_ready(&differ).unwrap_err().starts_with("site 2's new-slot fingerprints"));
        assert!(ssh_slot_ready(&[]).is_err());
    }
}
