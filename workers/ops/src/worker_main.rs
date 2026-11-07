//! The Worker: `POST /mint` from the edge's `OPS` binding. Answers 503 when the signing key cannot be read, so the
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

#[event(fetch)]
async fn fetch(mut req: Request, env: Env, _ctx: Context) -> Result<Response> {
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
