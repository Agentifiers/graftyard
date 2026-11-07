//! `graftyard admin bootstrap` and `graftyard mint-key rotate | step` (`design/DESIGN.md`, "Install operations",
//! First admin and the rotation table's `MintWrapKey` row; `design/SHARED.md`, "Instance sign-in in installs").
//!
//! Neither command has an operator route to send its command to yet (`decisions/OPEN-QUESTIONS.md`, R14), so each
//! writes the command its leader decides as a request file, as `placement set` does, and the secret it drew to a new
//! file of mode 0600. No secret reaches stdout or argv.
//!
//! - `admin bootstrap` draws a sign-in link and plans `PutCode { kind: SignInLink }` on the link's code shard, as the
//!   operator. The edge's `POST /login/oauth/link` spends it once (`UseCode`) and starts a `SignInMethod::Link`
//!   session on the account's `Identity` chain. The leader sets its expiry, `SIGN_IN_LINK_TTL` after the entry.
//! - `mint-key rotate` draws a new signing key and a wrap key, writes the `MintWrapKey` secret's text, and plans the
//!   new key's `PutVerifyingKey` on `TokenShard(0)`.
//! - `mint-key step` says what the rotation does next (`rotation_step`): wait, switch the secret with the customer's
//!   Wrangler login, or retire the old `kid`, whose `PutVerifyingKey { retired }` it plans.

use std::io::Write as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::Path;

use graftyard_identity::stateless::{KeyEpoch, RotationStep, SigningSecret, rotation_step};
use graftyard_identity::{
    CodeKind, Ed25519Public, Kid, SIGN_IN_LINK_TTL, ScopeSet, TokensCmd, TokensOp, code_hash, shard_of_hash,
};
use graftyard_types::{AccountId, ChainId, CredentialShard, DbId, Timestamp, TokenShard};
use serde_json::{Value, json};

use crate::config::{Res, now, random};

/// The link's text: 32 characters of Crockford base32 from 160 random bits.
fn link_secret() -> String {
    const A: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let b: [u8; 20] = random();
    let mut v = 0u64;
    let mut bits = 0;
    let mut out = String::with_capacity(32);
    for x in b {
        v = (v << 8) | u64::from(x);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(A[((v >> bits) & 31) as usize] as char);
        }
    }
    out
}

/// Writes `text` to a file that must not exist yet, readable by its owner only.
fn write_secret(path: &Path, text: &str) -> Res<()> {
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| format!("{}: {e} (a secret file is never overwritten)", path.display()))?;
    f.write_all(text.as_bytes()).map_err(|e| format!("{}: {e}", path.display()))
}

fn write_request(path: &Path, v: &Value) -> Res<()> {
    std::fs::write(path, serde_json::to_string_pretty(v).expect("json") + "\n")
        .map_err(|e| format!("{}: {e}", path.display()))
}

fn tokens_request(chain: ChainId, op: &str, cmd: &TokensCmd, note: &str) -> Value {
    json!({ "kind": "graftyard.tokens.command", "chain": chain, "op": op, "cmd": cmd, "note": note })
}

/// The sign-in link's command, on the code shard of its hash, as the operator: only the operator may put a
/// `SignInLink` (`TokensOp::PutCode`).
pub fn bootstrap_request(account: AccountId, link: &str) -> (ChainId, TokensCmd) {
    let hash = code_hash(link.as_bytes());
    let cmd = TokensCmd {
        cred: CredentialShard::NONE,
        op: TokensOp::PutCode {
            hash,
            kind: CodeKind::SignInLink,
            client: None,
            scopes: ScopeSet(0),
            account: Some(account),
        },
    };
    (ChainId::Tokens(shard_of_hash(&hash)), cmd)
}

pub fn bootstrap(account: i64, link_out: &Path, out: &Path) -> Res<()> {
    let account = AccountId(DbId(account));
    let link = link_secret();
    let (chain, cmd) = bootstrap_request(account, &link);
    write_secret(link_out, &link)?;
    let note = format!(
        "Commit as the operator on `chain`. The link lives {} s from its entry and signs in once, by POST \
         /login/oauth/link with `link` in the body.",
        SIGN_IN_LINK_TTL.as_secs()
    );
    write_request(out, &tokens_request(chain, "PutCode (SignInLink)", &cmd, &note))?;
    println!("sign-in link for account {}: secret in {} (mode 0600)", account.0.0, link_out.display());
    println!(
        "no operator route yet (R14): wrote the PutCode request for the Tokens leader on {chain} to {}",
        out.display()
    );
    Ok(())
}

pub fn rotate(kid: u32, secret_out: &Path, out: &Path) -> Res<()> {
    let kid = Kid(kid);
    let seed = SigningSecret(random::<32>());
    let secret = graftyard_ops::wrap(random::<32>(), kid, random::<12>(), &seed);
    write_secret(secret_out, &serde_json::to_string(&secret).expect("json"))?;
    let key = seed.public();
    let cmd = TokensCmd { cred: CredentialShard::NONE, op: TokensOp::PutVerifyingKey { kid, key, retired: None } };
    let note = "Commit as the operator, then run `mint-key step` to learn when the secret may switch.";
    write_request(out, &tokens_request(ChainId::Tokens(TokenShard(0)), "PutVerifyingKey", &cmd, note))?;
    println!(
        "kid {}: public {}; MintWrapKey text in {} (mode 0600)",
        kid.0,
        crate::config::hex(&key.0),
        secret_out.display()
    );
    println!("wrote the PutVerifyingKey request for tokens:0 to {}", out.display());
    Ok(())
}

pub struct StepArgs<'a> {
    pub old_kid: u32,
    pub old_public: &'a str,
    pub new_kid: u32,
    pub new_published: u64,
    pub switched: Option<u64>,
    pub out: &'a Path,
}

/// What the rotation does next at `at`, and the retire request when it is time for one.
pub fn step_at(a: &StepArgs<'_>, at: Timestamp) -> Res<(RotationStep, Option<TokensCmd>)> {
    let key = crate::config::parse_public_key(a.old_public).map(|k| Ed25519Public(k.0))?;
    let old = KeyEpoch { kid: Kid(a.old_kid), published: Timestamp(0), switched: a.switched.map(Timestamp) };
    let new = KeyEpoch { kid: Kid(a.new_kid), published: Timestamp(a.new_published), switched: None };
    let step = rotation_step(&old, &new, at);
    let cmd = match step {
        RotationStep::Retire { kid, at } => Some(TokensCmd {
            cred: CredentialShard::NONE,
            op: TokensOp::PutVerifyingKey { kid, key, retired: Some(at) },
        }),
        _ => None,
    };
    Ok((step, cmd))
}

pub fn step(a: &StepArgs<'_>) -> Res<()> {
    match step_at(a, now())? {
        (RotationStep::Wait, _) => {
            let at = a.new_published + graftyard_identity::stateless::INSTALLATION_TOKEN_TTL.as_millis() as u64;
            println!("wait: kid {} signs from {at} ms, one token lifetime after its row committed", a.new_kid);
        }
        (RotationStep::SignWithNew, _) => println!(
            "switch now: write kid {}'s MintWrapKey text with the customer's Wrangler login (`wrangler \
             secrets-store secret update <store> --secret-id <id> --remote`, the value at its prompt), then rerun \
             with --switched <ms>",
            a.new_kid
        ),
        (RotationStep::Retire { kid, at }, Some(cmd)) => {
            let note = "Commit as the operator; the key verifies until `retired`.";
            write_request(
                a.out,
                &tokens_request(ChainId::Tokens(TokenShard(0)), "PutVerifyingKey (retire)", &cmd, note),
            )?;
            println!("retire kid {} at {} ms: wrote the request for tokens:0 to {}", kid.0, at.0, a.out.display());
        }
        (RotationStep::Retire { .. }, None) => unreachable!("a retire step plans its command"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use graftyard_identity::stateless::{INSTALLATION_TOKEN_TTL, SIGNING_KEY_CACHE};
    use graftyard_identity::{IdentityDep, NamesEvent, NamesFold, TokensErr, TokensEvent, TokensFold, account_shard};
    use graftyard_raft::{Command, Fold, Record};

    use super::*;

    fn decide(fold: &mut TokensFold, cmd: &TokensCmd) -> Result<Vec<TokensEvent>, TokensErr> {
        // A user's command reads its `Account` row; give it one that is live, so only the op's own rule refuses.
        let deps: Vec<(ChainId, IdentityDep)> = cmd
            .cred
            .account
            .map(|a| {
                let mut n = NamesFold::default();
                n.apply(&NamesEvent::Account { id: a, suspended: false, sessions_not_before: Timestamp(0) });
                (ChainId::Names(account_shard(a)), IdentityDep::Names(n))
            })
            .into_iter()
            .collect();
        let s = fold.scratch();
        let (evs, ()) = cmd.decide(&s, &deps)?;
        drop(s);
        for e in &evs {
            fold.apply(e);
        }
        Ok(evs.into_vec())
    }

    #[test]
    fn a_bootstrap_link_is_an_operator_sign_in_link_on_its_own_code_shard() {
        let link = link_secret();
        assert_eq!(link.len(), 32);
        assert_ne!(link, link_secret(), "each link is drawn afresh");
        let account = AccountId(DbId(7_100_003));
        let (chain, cmd) = bootstrap_request(account, &link);
        let ChainId::Tokens(shard) = chain else { panic!("a Tokens chain") };
        assert_eq!(shard, cmd.shard(), "the request names the shard its leader checks");
        assert!(cmd.cred.is_operator());
        let mut bytes = Vec::new();
        cmd.encode(&mut bytes);
        assert!(!bytes.is_empty());
        // The leader accepts it once, and the link names the account with no client.
        let mut f = TokensFold::new(shard);
        let evs = decide(&mut f, &cmd).unwrap();
        assert!(
            matches!(&evs[..], [TokensEvent::Code { kind: CodeKind::SignInLink, account: Some(a), client: None, .. }] if *a == account)
        );
        assert_eq!(decide(&mut f, &cmd), Err(TokensErr::Exists));
        // Under a user's credential it is refused: only the operator puts a sign-in link.
        let as_user = TokensCmd { cred: CredentialShard { account: Some(account), key: None }, ..cmd };
        assert_eq!(decide(&mut TokensFold::new(shard), &as_user), Err(TokensErr::Forbidden));
    }

    #[test]
    fn mint_key_step_waits_one_lifetime_then_retires_after_the_cache_and_a_lifetime() {
        let dir = std::env::temp_dir().join(format!("graftyard-mintkey-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (secret, req) = (dir.join("kid2.json"), dir.join("kid2.request.json"));
        rotate(2, &secret, &req).unwrap();
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(std::fs::metadata(&secret).unwrap().permissions().mode() & 0o777, 0o600);
        let (kid, seed) = graftyard_ops::unwrap(&std::fs::read_to_string(&secret).unwrap()).unwrap();
        assert_eq!(kid, Kid(2));
        let r: Value = serde_json::from_str(&std::fs::read_to_string(&req).unwrap()).unwrap();
        let cmd: TokensCmd = serde_json::from_value(r["cmd"].clone()).unwrap();
        assert_eq!(cmd.op, TokensOp::PutVerifyingKey { kid: Kid(2), key: seed.public(), retired: None });
        assert_eq!(r["chain"], json!("tokens:0"));
        assert!(rotate(2, &secret, &req).is_err(), "a secret file is never overwritten");

        let old_public = crate::config::hex(&SigningSecret([1; 32]).public().0);
        let h = INSTALLATION_TOKEN_TTL.as_millis() as u64;
        let mut a = StepArgs {
            old_kid: 1,
            old_public: &old_public,
            new_kid: 2,
            new_published: 1_000,
            switched: None,
            out: &req,
        };
        assert_eq!(step_at(&a, Timestamp(1_000 + h - 1)).unwrap().0, RotationStep::Wait);
        assert_eq!(step_at(&a, Timestamp(1_000 + h)).unwrap().0, RotationStep::SignWithNew);
        a.switched = Some(1_000 + h);
        let (step, cmd) = step_at(&a, Timestamp(1_000 + h)).unwrap();
        let RotationStep::Retire { kid, at } = step else { panic!("{step:?}") };
        assert_eq!(kid, Kid(1));
        assert!(at.0 >= 1_000 + h + SIGNING_KEY_CACHE.as_millis() as u64 + h, "after the cache and a lifetime");
        // The retire keeps the key's bytes, so the leader takes it as the same key gaining `retired`.
        let mut f = TokensFold::new(TokenShard(0));
        let publish1 = TokensCmd {
            cred: CredentialShard::NONE,
            op: TokensOp::PutVerifyingKey { kid: Kid(1), key: SigningSecret([1; 32]).public(), retired: None },
        };
        decide(&mut f, &publish1).unwrap();
        decide(&mut f, &cmd.unwrap()).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
