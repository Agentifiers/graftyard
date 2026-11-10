//! The Worker: `POST /mint` and `POST /wrap` from the edge's `OPS` binding. Answers 503 when the signing key cannot be read, so the
//! edge mints nothing then.

use std::cell::RefCell;

use graftyard_identity::stateless::{Grant, Minter};
use graftyard_types::Timestamp;
use serde_json::json;
use worker::*;

use crate::MintKeySource;

thread_local! {
    /// One isolate's minter: the unwrapped key, held for at most `SIGNING_KEY_CACHE`.
    static MINTER: RefCell<Minter> = RefCell::new(Minter::default());
}

fn rand10() -> [u8; 10] {
    let mut b = [0u8; 10];
    let crypto: web_sys::Crypto = js_sys::Reflect::get(&js_sys::global(), &"crypto".into()).unwrap().into();
    crypto.get_random_values_with_u8_array(&mut b).unwrap();
    b
}

/// `POST /wrap` (M4-WP1): a hook secret the edge received, sealed under `SecretsWrapKey`'s current slot. The two
/// slots are the Secrets Store bindings `SECRETS_WRAP_KEY_A` and `_B`, and `SECRETS_WRAP_KEY_CURRENT` names the one new
/// values take [inference: the design names the secret, not its bindings]. Nothing here logs the body.
async fn wrap(mut req: Request, env: &Env) -> Result<Response> {
    use graftyard_identity::sealed::HookSecrets as _;
    #[derive(serde::Deserialize)]
    struct Body {
        secret: String,
    }
    let Ok(b) = req.json::<Body>().await else { return Response::error("not a hook secret", 400) };
    let mut texts = Vec::new();
    for name in ["SECRETS_WRAP_KEY_A", "SECRETS_WRAP_KEY_B"] {
        if let Ok(s) = env.secret_store(name)
            && let Some(t) = s.get().await?
        {
            texts.push(t);
        }
    }
    let current = env.var("SECRETS_WRAP_KEY_CURRENT").ok().and_then(|v| v.to_string().parse().ok()).unwrap_or(0);
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    let src = match crate::WrapKeySource::parse(current, &refs) {
        Ok(s) => s,
        Err(e) => return Response::error(e.to_string(), 503),
    };
    let mut nonce = [0u8; 12];
    nonce.copy_from_slice(&rand10().into_iter().chain(rand10()).take(12).collect::<Vec<_>>());
    match src.wrap(b.secret.as_bytes(), nonce) {
        Ok(w) => Response::from_json(&w),
        Err(e) => Response::error(e.to_string(), 503),
    }
}

#[event(fetch)]
async fn fetch(mut req: Request, env: Env, _ctx: Context) -> Result<Response> {
    if req.method() == Method::Post && req.path() == "/wrap" {
        return wrap(req, &env).await;
    }
    if req.method() != Method::Post || req.path() != "/mint" {
        return Response::error("Not Found", 404);
    }
    let grant: Grant = match req.json().await {
        Ok(g) => g,
        Err(e) => return Response::error(format!("not a Grant: {e}"), 400),
    };
    let now = Timestamp(Date::now().as_millis());
    // The secret is read only when the minter's key is missing or `SIGNING_KEY_CACHE` old.
    let text = if MINTER.with(|m| m.borrow().needs_key(now)) {
        match env.secret_store("MINT_WRAP_KEY")?.get().await? {
            Some(t) => t,
            None => return Response::error("MintWrapKey is not set", 503),
        }
    } else {
        String::new()
    };
    let minted = MINTER.with(|m| m.borrow_mut().mint(&MintKeySource(&text), &grant, now, rand10()));
    match minted {
        Ok((token, claims)) => Response::from_json(&json!({
            "token": token,
            "expires_at": graftyard_types::Timestamp(claims.exp.0).0,
            "jti": claims.jti.0.to_string(),
            "kid": claims.kid.0,
            "fetches": MINTER.with(|m| m.borrow().fetches),
        })),
        Err(e) => Response::error(e.to_string(), 503),
    }
}
