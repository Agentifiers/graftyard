//! `graftyard vault token` (`design/DESIGN.md`, Shape, "Lifeboat feed", `ExportClaims`; `build/BUILD-PLAN.md`,
//! M5-WP5): signs an export token offline with the customer key, for the install in install.toml.
//!
//! The token is written to a file of mode 0600 and never reaches stdout, stderr, or argv; neither does the key. The
//! command prints only the token's `jti`, scope, holder, expiry, and the file it wrote. The token revokes itself with
//! `DELETE /api/graftyard/v1/export/token`.

use std::io::Write as _;
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::Path;

use graftyard_identity::export_token::{self, EXPORT_TOKEN_MAX, ExportClaims, HOLDER_MAX};
use graftyard_release::{InstallId, Scope, public_key};
use graftyard_types::{Timestamp, Ulid};

use crate::config::{InstallToml, Res, now, random, read_private_key};

const DAY_MS: u64 = 24 * 3600 * 1000;

/// What `vault token` takes besides the install file.
pub struct TokenArgs<'a> {
    pub key: &'a Path,
    pub scope: &'a str,
    pub holder: &'a str,
    pub days: u64,
    pub out: &'a Path,
}

/// `install`, `owner:<id>`, or `repo:<id>`, as the token spells it.
fn parse_scope(s: &str) -> Res<Scope> {
    export_token::parse_scope(s).ok_or_else(|| format!("--scope {s:?}: expected install, owner:<id>, or repo:<id>"))
}

/// The claims of a token issued at `at` (whole seconds, as the token carries them), checked against the token's
/// bounds: a holder of 1 to `HOLDER_MAX` bytes, and 1 day up to `EXPORT_TOKEN_MAX`.
fn claims(install: InstallId, scope: Scope, holder: &str, days: u64, at: Timestamp, jti: Ulid) -> Res<ExportClaims> {
    let max_days = EXPORT_TOKEN_MAX.as_millis() as u64 / DAY_MS;
    if days == 0 || days > max_days {
        return Err(format!("--days {days}: an export token lives 1 to {max_days} days"));
    }
    if holder.is_empty() || holder.len() > HOLDER_MAX {
        return Err(format!("--holder: 1 to {HOLDER_MAX} bytes"));
    }
    let iat = Timestamp(at.0 / 1000 * 1000);
    Ok(ExportClaims { install, scope, holder: holder.into(), jti, iat, exp: Timestamp(iat.0 + days * DAY_MS) })
}

/// Writes `text` to `path`, created or truncated, readable by its owner only.
fn write_token(path: &Path, text: &str) -> Res<()> {
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    // `mode` applies only to a file it creates: an existing file loses any wider mode before the token goes in.
    f.set_permissions(std::fs::Permissions::from_mode(0o600)).map_err(|e| format!("{}: {e}", path.display()))?;
    f.write_all(format!("{text}\n").as_bytes()).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn token(config: &Path, a: &TokenArgs) -> Res<()> {
    let anchor = InstallToml::load(config)?.anchor()?;
    let scope = parse_scope(a.scope)?;
    let seed = read_private_key(a.key)?;
    let at = now();
    let c = claims(anchor.install, scope, a.holder, a.days, at, Ulid::new(at, random()))?;
    let text = export_token::sign(&c, &seed);
    let customer = public_key(&seed);
    match export_token::verify(&text, anchor.install, &customer, at) {
        Ok(v) if v == c => {}
        Ok(_) => return Err("the signed token verifies to other claims".into()),
        Err(e) => return Err(format!("the signed token does not verify: {e}")),
    }
    if customer != anchor.customer {
        eprintln!(
            "note: {} is not install.toml's customer key; the edge takes the token only if a later Keys entry made it \
             the customer key",
            a.key.display()
        );
    }
    write_token(a.out, &text)?;
    println!(
        "export token jti {} scope {} holder {:?} exp {} ({} days): wrote {} (mode 0600)",
        c.jti,
        export_token::scope_text(c.scope),
        &*c.holder,
        c.exp.0 / 1000,
        a.days,
        a.out.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{hex, mint_install};

    const SEED: [u8; 32] = [9; 32];

    fn setup(name: &str) -> (std::path::PathBuf, InstallId) {
        let dir = std::env::temp_dir().join(format!("graftyard-vault-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let install = mint_install();
        let t = InstallToml {
            install: install.to_string(),
            account: "be2a".into(),
            zone: "git.acme.example".into(),
            customer_key: public_key(&SEED).hex(),
            evaluation: false,
            genesis: None,
            sites: Vec::new(),
            rtt: Vec::new(),
            default_placement: None,
        };
        t.save(&dir.join("install.toml")).unwrap();
        std::fs::write(dir.join("customer.key"), hex(&SEED) + "\n").unwrap();
        (dir, install)
    }

    #[test]
    fn a_written_token_verifies_with_the_asked_scope_holder_and_expiry() {
        let (dir, install) = setup("ok");
        let out = dir.join("export.token");
        // An existing, world-readable file is truncated and narrowed to 0600.
        std::fs::write(&out, "old contents that are longer than nothing").unwrap();
        std::fs::set_permissions(&out, std::fs::Permissions::from_mode(0o644)).unwrap();
        let before = now();
        let args =
            TokenArgs { key: &dir.join("customer.key"), scope: "owner:42", holder: "acme-org", days: 30, out: &out };
        token(&dir.join("install.toml"), &args).unwrap();
        assert_eq!(std::fs::metadata(&out).unwrap().permissions().mode() & 0o777, 0o600);
        let text = std::fs::read_to_string(&out).unwrap();
        let c = export_token::verify(text.trim_end(), install, &public_key(&SEED), now()).unwrap();
        assert_eq!(c.scope, Scope::Owner(graftyard_types::AccountId(graftyard_types::DbId(42))));
        assert_eq!(&*c.holder, "acme-org");
        assert_eq!(c.exp.0 - c.iat.0, 30 * DAY_MS);
        assert!(c.iat.0 + 1000 > before.0 && c.iat <= now());
        assert_eq!(c.jti.time().0 / 1000, c.iat.0 / 1000, "the jti is minted at issue");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn refuses_401_days_a_bad_scope_and_a_bad_holder_and_writes_nothing() {
        let (dir, _) = setup("refuse");
        let out = dir.join("export.token");
        let key = dir.join("customer.key");
        let config = dir.join("install.toml");
        let run = |scope: &str, holder: &str, days: u64| {
            token(&config, &TokenArgs { key: &key, scope, holder, days, out: &out })
        };
        assert!(run("install", "vault", 401).unwrap_err().contains("--days 401"));
        assert!(run("install", "vault", 0).is_err());
        assert!(run("org:1", "vault", 365).unwrap_err().contains("--scope"));
        assert!(run("repo:", "vault", 365).is_err());
        assert!(run("install", "", 365).is_err());
        assert!(run("install", &"x".repeat(HOLDER_MAX + 1), 365).is_err());
        assert!(!out.exists());
        run("repo:7", "vault", 400).unwrap();
        assert!(out.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
