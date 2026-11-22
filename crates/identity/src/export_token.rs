//! Export tokens (`design/DESIGN.md`, Shape, "Lifeboat feed", `ExportClaims`; `build/BUILD-PLAN.md`, M5-WP5).
//!
//! An export token is the claims as JSON, base64url with no padding, a `.`, and the customer key's Ed25519 signature
//! over that first part, base64url too. `graftyard vault init` and `graftyard vault token` sign one offline; the edge
//! verifies it against the customer key of its cached `Keys` (`verify`) and serves only the chains `covers` allows.
//! It is revoked like an installation token: `revoke_command` is `RevokeJti` on `jti_shard(jti)` under the token's
//! own `CredentialKey::Jti`, which `DELETE /api/graftyard/v1/export/token` commits, and export reads honor that row
//! within `READ_CACHE` as `ghs_` reads do.

use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use ed25519_dalek::{Signer as _, SigningKey, VerifyingKey};
use graftyard_release::{InstallId, KeyId, Scope};
use graftyard_types::{AccountId, ChainId, CredentialKey, CredentialShard, DbId, RepoId, Timestamp, Ulid};
use serde::{Deserialize, Serialize};

use crate::stateless::CLOCK_SKEW;
use crate::tokens::{Jti, TokensCmd, TokensOp, jti_shard};

/// DESIGN's bound on an export token's life [guess].
pub const EXPORT_TOKEN_MAX: Duration = Duration::from_secs(400 * 24 * 3600);
/// The longest `holder` a token may name.
pub const HOLDER_MAX: usize = 64;

/// DESIGN's `ExportClaims`. `iat` and `exp` are whole seconds in the token, as a JWT carries them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportClaims {
    pub install: InstallId,
    pub scope: Scope,
    /// `"vault"`, or the org a managed tenant's token is for.
    pub holder: Box<str>,
    pub jti: Ulid,
    pub iat: Timestamp,
    pub exp: Timestamp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ExportTokenError {
    #[error("not claims, a dot, and a signature")]
    Shape,
    #[error("the claims do not parse")]
    Claims,
    #[error("the token is for another install")]
    Install,
    #[error("bad signature")]
    Signature,
    #[error("expired")]
    Expired,
    #[error("issued in the future")]
    NotYet,
    #[error("lives longer than an export token may")]
    Lifetime,
}

/// The token's JSON. `scope` is `"install"`, `"owner:<id>"`, or `"repo:<id>"`.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Payload {
    install: String,
    scope: String,
    holder: String,
    jti: String,
    iat: u64,
    exp: u64,
}

/// A scope as the token spells it: `install`, `owner:<id>`, or `repo:<id>`.
pub fn scope_text(s: Scope) -> String {
    match s {
        Scope::Install => "install".into(),
        Scope::Owner(a) => format!("owner:{}", a.0.0),
        Scope::Repo(r) => format!("repo:{}", r.0.0),
    }
}

/// `scope_text`'s inverse.
pub fn parse_scope(s: &str) -> Option<Scope> {
    let id = |v: &str| (!v.is_empty() && v.bytes().all(|b| b.is_ascii_digit())).then(|| v.parse().ok()).flatten();
    match s.split_once(':') {
        None if s == "install" => Some(Scope::Install),
        Some(("owner", v)) => Some(Scope::Owner(AccountId(DbId(id(v)?)))),
        Some(("repo", v)) => Some(Scope::Repo(RepoId(DbId(id(v)?)))),
        _ => None,
    }
}

/// Signs `claims` with the customer key's 32-byte seed: what `graftyard vault token` runs on the customer's machine.
pub fn sign(claims: &ExportClaims, customer_seed: &[u8; 32]) -> String {
    let p = Payload {
        install: claims.install.to_string(),
        scope: scope_text(claims.scope),
        holder: claims.holder.to_string(),
        jti: claims.jti.to_string(),
        iat: claims.iat.0 / 1000,
        exp: claims.exp.0 / 1000,
    };
    let head = B64.encode(serde_json::to_vec(&p).expect("json"));
    let sig = SigningKey::from_bytes(customer_seed).sign(head.as_bytes());
    format!("{head}.{}", B64.encode(sig.to_bytes()))
}

/// The claims of `text` once its signature verifies under `customer`, it names `install`, and it is inside its window
/// at `now`. Revocation is the caller's: the `jti`'s row on `jti_shard`, read as `ghs_` reads it.
pub fn verify(
    text: &str,
    install: InstallId,
    customer: &KeyId,
    now: Timestamp,
) -> Result<ExportClaims, ExportTokenError> {
    let (head, sig) = text.split_once('.').ok_or(ExportTokenError::Shape)?;
    let sig: [u8; 64] = B64.decode(sig).ok().and_then(|b| b.try_into().ok()).ok_or(ExportTokenError::Shape)?;
    let vk = VerifyingKey::from_bytes(&customer.0).map_err(|_| ExportTokenError::Signature)?;
    vk.verify_strict(head.as_bytes(), &ed25519_dalek::Signature::from_bytes(&sig))
        .map_err(|_| ExportTokenError::Signature)?;
    let p: Payload =
        B64.decode(head).ok().and_then(|b| serde_json::from_slice(&b).ok()).ok_or(ExportTokenError::Claims)?;
    let claims = ExportClaims {
        install: p.install.parse().map_err(|_| ExportTokenError::Claims)?,
        scope: parse_scope(&p.scope).ok_or(ExportTokenError::Claims)?,
        holder: (!p.holder.is_empty() && p.holder.len() <= HOLDER_MAX)
            .then(|| p.holder.into_boxed_str())
            .ok_or(ExportTokenError::Claims)?,
        jti: p.jti.parse().map_err(|_| ExportTokenError::Claims)?,
        iat: Timestamp(p.iat.checked_mul(1000).ok_or(ExportTokenError::Claims)?),
        exp: Timestamp(p.exp.checked_mul(1000).ok_or(ExportTokenError::Claims)?),
    };
    if claims.install != install {
        return Err(ExportTokenError::Install);
    }
    if claims.exp.0.saturating_sub(claims.iat.0) > EXPORT_TOKEN_MAX.as_millis() as u64 {
        return Err(ExportTokenError::Lifetime);
    }
    if claims.iat.0 > now.0 + CLOCK_SKEW.as_millis() as u64 {
        return Err(ExportTokenError::NotYet);
    }
    if claims.exp <= now {
        return Err(ExportTokenError::Expired);
    }
    Ok(claims)
}

/// Whether an export under `scope` covers `chain`. An install-scoped export covers every `ChainId`, `Release`,
/// `Names`, `Tokens`, and `Ids` included (`design/DESIGN.md`, "Install operations", Vault); a narrower scope covers
/// what a `StaleMode` flag of that scope covers (`graftyard_release::view::covers`), so an org's export never reaches
/// a system chain. `owner` is the repository chain's owner where the caller knows it.
pub fn covers(scope: Scope, chain: ChainId, owner: Option<AccountId>) -> bool {
    scope == Scope::Install || graftyard_release::view::covers(scope, chain, owner)
}

impl ExportClaims {
    pub fn jti(&self) -> Jti {
        Jti(self.jti)
    }
}

/// `DELETE /api/graftyard/v1/export/token`: the token revokes its own `jti` until its `exp`, as `DELETE
/// /installation/token` does. The leader accepts it only under that `jti`'s credential, or from `graft-ops`.
pub fn revoke_command(claims: &ExportClaims) -> TokensCmd {
    let jti = claims.jti();
    TokensCmd {
        cred: CredentialShard { account: None, key: Some(CredentialKey::Jti { shard: jti_shard(&jti), jti: jti.0 }) },
        op: TokensOp::RevokeJti { jti, exp: claims.exp },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEED: [u8; 32] = [7; 32];
    const HOUR: u64 = 3_600_000;

    fn install() -> InstallId {
        InstallId(Ulid(0x0190_0000_0000_0000_0000_0000_0000_0001))
    }

    fn customer() -> KeyId {
        graftyard_release::public_key(&SEED)
    }

    fn claims(scope: Scope) -> ExportClaims {
        ExportClaims {
            install: install(),
            scope,
            holder: "vault".into(),
            jti: Ulid(0x0190_0000_0000_0000_0000_0000_00ab_cdef),
            iat: Timestamp(1_000 * HOUR),
            exp: Timestamp(1_000 * HOUR + 30 * 24 * HOUR),
        }
    }

    #[test]
    fn a_signed_token_verifies_to_its_claims_for_every_scope() {
        let now = Timestamp(1_001 * HOUR);
        for scope in [Scope::Install, Scope::Owner(AccountId(DbId(42))), Scope::Repo(RepoId(DbId(9)))] {
            let c = claims(scope);
            assert_eq!(verify(&sign(&c, &SEED), install(), &customer(), now), Ok(c));
        }
    }

    #[test]
    fn refuses_another_key_another_install_a_flipped_byte_and_the_window() {
        let c = claims(Scope::Install);
        let t = sign(&c, &SEED);
        let now = Timestamp(1_001 * HOUR);
        let other = graftyard_release::public_key(&[8; 32]);
        assert_eq!(verify(&t, install(), &other, now), Err(ExportTokenError::Signature));
        let elsewhere = InstallId(Ulid(install().0.0 + 1));
        assert_eq!(verify(&t, elsewhere, &customer(), now), Err(ExportTokenError::Install));
        // A claims byte changed after signing: the scope widened from an org to the install.
        let narrow = sign(&claims(Scope::Owner(AccountId(DbId(42)))), &SEED);
        let (head, sig) = narrow.split_once('.').unwrap();
        let json = String::from_utf8(B64.decode(head).unwrap()).unwrap().replace("owner:42", "install");
        let forged = format!("{}.{sig}", B64.encode(json));
        assert_eq!(verify(&forged, install(), &customer(), now), Err(ExportTokenError::Signature));
        assert_eq!(verify(&t, install(), &customer(), c.exp), Err(ExportTokenError::Expired));
        assert!(verify(&t, install(), &customer(), Timestamp(c.exp.0 - 1000)).is_ok());
        let early = Timestamp(c.iat.0 - CLOCK_SKEW.as_millis() as u64 - 1000);
        assert_eq!(verify(&t, install(), &customer(), early), Err(ExportTokenError::NotYet));
        assert_eq!(verify("no-dot", install(), &customer(), now), Err(ExportTokenError::Shape));
    }

    #[test]
    fn lifetime_is_at_most_400_days() {
        let mut c = claims(Scope::Install);
        c.exp = Timestamp(c.iat.0 + EXPORT_TOKEN_MAX.as_millis() as u64);
        let now = Timestamp(c.iat.0 + 1000);
        assert!(verify(&sign(&c, &SEED), install(), &customer(), now).is_ok());
        c.exp = Timestamp(c.exp.0 + 1000);
        assert_eq!(verify(&sign(&c, &SEED), install(), &customer(), now), Err(ExportTokenError::Lifetime));
    }

    #[test]
    fn install_scope_covers_release_and_system_chains_and_an_org_scope_does_not() {
        let org = AccountId(DbId(42));
        let system = [
            ChainId::Release,
            ChainId::Ids,
            ChainId::Names(graftyard_types::NameShard(3)),
            ChainId::Tokens(graftyard_types::TokenShard(5)),
        ];
        for c in system {
            assert!(covers(Scope::Install, c, None), "{c}");
            assert!(!covers(Scope::Owner(org), c, None), "{c}");
        }
        let repo = ChainId::Repo(RepoId(DbId(9)));
        assert!(covers(Scope::Owner(org), repo, Some(org)));
        assert!(!covers(Scope::Owner(org), repo, Some(AccountId(DbId(43)))));
        assert!(covers(Scope::Repo(RepoId(DbId(9))), repo, None));
        assert!(!covers(Scope::Repo(RepoId(DbId(10))), repo, None));
    }

    #[test]
    fn the_revoke_command_is_accepted_under_its_own_jti_and_honoured() {
        use graftyard_raft::{Command as _, Fold as _, TierSet};

        use crate::tokens::{TokensErr, TokensEvent, TokensFold, TokensRead as _};

        let c = claims(Scope::Install);
        let cmd = revoke_command(&c);
        let shard = jti_shard(&c.jti());
        let mut fold = TokensFold::new(shard);
        let now = Timestamp(1_001 * HOUR);
        let (events, ()) = cmd.decide(&fold.scratch_at(now, TierSet::CORE), &[]).expect("its own jti may revoke it");
        assert_eq!(events.as_slice(), [TokensEvent::JtiRevoked { jti: c.jti(), exp: c.exp }]);
        for e in &events {
            fold.apply(e);
        }
        assert!(fold.jti_revoked(&c.jti(), now));
        assert!(!fold.jti_revoked(&c.jti(), c.exp), "the row lapses at the token's exp");
        // The command is the token's, never an operator's: once revoked, the token cannot send it again.
        let again = cmd.decide(&fold.scratch_at(now, TierSet::CORE), &[]);
        assert!(matches!(again, Err(TokensErr::Unauthorized(_))), "{:?}", again.map(|_| ()));
        // Another export token may not revoke it (refused before the op: its jti is on another shard).
        let mut other = revoke_command(&c);
        other.cred.key = Some(CredentialKey::Jti { shard, jti: Ulid(c.jti.0 + 1) });
        let r = other.decide(&TokensFold::new(shard).scratch_at(now, TierSet::CORE), &[]);
        assert!(matches!(r, Err(TokensErr::Unauthorized(_) | TokensErr::Forbidden)), "{:?}", r.map(|_| ()));
    }
}
