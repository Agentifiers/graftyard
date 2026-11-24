//! The Worker: `POST /mint` and `POST /wrap` from the edge's `OPS` binding, and `POST /unwrap` from the site scripts'. Answers 503 when the signing key cannot be read, so the
//! edge mints nothing then. The Cron Trigger runs the executor (`crate::exec`) when the version binds `OPS_SCRIPTS`.

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

/// The `SecretsWrapKey` slots: the Secrets Store bindings `SECRETS_WRAP_KEY_A` (slot 0) and `_B` (slot 1), both
/// bound and both unwrapping, and the slot new values wrap under [inference: the design names the secret, not its
/// bindings]. That slot is the one a `Rotate` switched to (`ReleaseFold::slots`) once the view holds an entry for it;
/// before any, or while a script that binds the members holds no view yet, it is `SECRETS_WRAP_KEY_CURRENT` (default
/// 0), so an install that set the variable before its first `Rotate` keeps its slot. The variable names a slot both
/// bindings still hold, so wrapping under it is safe even when the view would name the other: either unwraps.
async fn wrap_keys(env: &Env, ctx: &Context) -> std::result::Result<crate::WrapKeySource, String> {
    let mut texts = Vec::new();
    for name in crate::WRAP_KEY_BINDINGS {
        if let Ok(s) = env.secret_store(name)
            && let Some(t) = s.get().await.map_err(|e| e.to_string())?
        {
            texts.push(t);
        }
    }
    let switched =
        view_fold(env, ctx, |f| f.slots.get(&graftyard_release::SecretName::SecretsWrapKey).map(|(s, _)| *s)).await;
    // A switched slot only; no members, no view yet, or no `Rotate` so far all read the variable.
    let current = match switched {
        Some(Some(Some(s))) => s,
        _ => env.var("SECRETS_WRAP_KEY_CURRENT").ok().and_then(|v| v.to_string().parse().ok()).unwrap_or(0),
    };
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    crate::WrapKeySource::parse(current, &refs).map_err(|e| e.to_string())
}

/// `POST /wrap` (M4-WP1): a hook secret the edge received, sealed under `SecretsWrapKey`'s current slot. Nothing here
/// logs the body.
async fn wrap(mut req: Request, env: &Env, ctx: &Context) -> Result<Response> {
    use graftyard_identity::sealed::HookSecrets as _;
    #[derive(serde::Deserialize)]
    struct Body {
        secret: String,
    }
    let Ok(b) = req.json::<Body>().await else { return Response::error("not a hook secret", 400) };
    let src = match wrap_keys(env, ctx).await {
        Ok(s) => s,
        Err(e) => return Response::error(e, 503),
    };
    let mut nonce = [0u8; 12];
    nonce.copy_from_slice(&rand10().into_iter().chain(rand10()).take(12).collect::<Vec<_>>());
    match src.wrap(b.secret.as_bytes(), nonce) {
        Ok(w) => Response::from_json(&w),
        Err(e) => Response::error(e.to_string(), 503),
    }
}

/// `POST /unwrap` (M4-WP1), from a site's `HookSequencer` over its `OPS` binding: the hook secret a `Wrapped` seals,
/// as hex, which the sequencer signs attempts with and keeps only in its isolate's memory (`design/DESIGN.md`, Shape:
/// "Only `graft-ops` reads `SecretsWrapKey`: it ... unwraps hook secrets for `HookSequencer`"). The script has no
/// public route, so only a binding reaches it. Nothing here logs the body or the answer.
async fn unwrap(mut req: Request, env: &Env, ctx: &Context) -> Result<Response> {
    use graftyard_identity::sealed::HookSecrets as _;
    let Ok(w) = req.json::<graftyard_identity::sealed::Wrapped>().await else {
        return Response::error("not a wrapped secret", 400);
    };
    let src = match wrap_keys(env, ctx).await {
        Ok(s) => s,
        Err(e) => return Response::error(e, 503),
    };
    match src.unwrap(&w) {
        Ok(s) => Response::from_json(&json!({ "secret": s.0.iter().map(|b| format!("{b:02x}")).collect::<String>() })),
        Err(e) => Response::error(e.to_string(), 422),
    }
}

struct WebRng;

impl crypto_box::aead::rand_core::RngCore for WebRng {
    fn next_u32(&mut self) -> u32 {
        let mut b = [0u8; 4];
        self.fill_bytes(&mut b);
        u32::from_le_bytes(b)
    }
    fn next_u64(&mut self) -> u64 {
        let mut b = [0u8; 8];
        self.fill_bytes(&mut b);
        u64::from_le_bytes(b)
    }
    fn fill_bytes(&mut self, d: &mut [u8]) {
        let crypto: web_sys::Crypto = js_sys::Reflect::get(&js_sys::global(), &"crypto".into()).unwrap().into();
        crypto.get_random_values_with_u8_array(d).unwrap();
    }
    fn try_fill_bytes(&mut self, d: &mut [u8]) -> std::result::Result<(), crypto_box::aead::rand_core::Error> {
        self.fill_bytes(d);
        Ok(())
    }
}

impl crypto_box::aead::rand_core::CryptoRng for WebRng {}

/// M4-WP3 (BF-59), from a site's `OPS` binding or the edge's: `POST /actions/sealkey` mints an owner's `SealKey`;
/// `POST /actions/open` answers one job's secret plaintexts (`crate::OpenRequest`), to `acquirejob` only; `POST
/// /actions/reseal` reseals records to a new key for rotation and answers no plaintext; `POST /actions/rewrap` wraps
/// a hook secret again under the current slot. Nothing here logs a body or an answer.
async fn actions(mut req: Request, env: &Env, ctx: &Context, path: &str) -> Result<Response> {
    use crypto_box::aead::rand_core::RngCore as _;
    let src = match wrap_keys(env, ctx).await {
        Ok(s) => s,
        Err(e) => return Response::error(e, 503),
    };
    match path {
        "/actions/sealkey" => {
            let (mut seed, mut nonce) = ([0u8; 32], [0u8; 12]);
            WebRng.fill_bytes(&mut seed);
            WebRng.fill_bytes(&mut nonce);
            match crate::mint_seal_key(&src, seed, nonce, graftyard_types::Timestamp(Date::now().as_millis())) {
                Ok(k) => Response::from_json(&k),
                Err(e) => Response::error(e.to_string(), 503),
            }
        }
        "/actions/open" => {
            let Ok(r) = req.json::<crate::OpenRequest>().await else {
                return Response::error("not an open request", 400);
            };
            let (secrets, failed) = crate::open_secrets(&src, &r);
            Response::from_json(&json!({ "secrets": secrets, "failed": failed }))
        }
        "/actions/reseal" => {
            #[derive(serde::Deserialize)]
            struct Body {
                to: graftyard_identity::sealed::SealKey,
                records: crate::OpenRequest,
            }
            let Ok(b) = req.json::<Body>().await else { return Response::error("not a reseal request", 400) };
            let (sealed, failed) = crate::reseal(&src, &b.records, &b.to, &mut WebRng);
            Response::from_json(&json!({ "sealed": sealed, "failed": failed }))
        }
        "/actions/rewrap" => {
            let Ok(w) = req.json::<graftyard_identity::sealed::Wrapped>().await else {
                return Response::error("not a wrapped value", 400);
            };
            let mut nonce = [0u8; 12];
            WebRng.fill_bytes(&mut nonce);
            match crate::rewrap(&src, &w, nonce) {
                Ok(w) => Response::from_json(&w),
                Err(e) => Response::error(e.to_string(), 422),
            }
        }
        _ => Response::error("Not Found", 404),
    }
}

/// `f` of this script's `Release` fold, refreshed as the edge refreshes its own (`view::ensure`), when the script
/// binds the members (`--executor`). `Some(None)` when it binds none, `None` when it binds them but holds no view yet.
async fn view_fold<R>(
    env: &Env,
    ctx: &Context,
    f: impl FnOnce(&graftyard_release::ReleaseFold) -> R,
) -> Option<Option<R>> {
    let (Ok(sites), Ok(install), Ok(customer)) =
        (env.var("GRAFTYARD_SITES"), env.var("GRAFTYARD_INSTALL"), env.var("GRAFTYARD_CUSTOMER_KEY"))
    else {
        return Some(None);
    };
    let sites = serde_json::from_str::<graftyard_types::SiteTable>(&sites.to_string()).ok()?;
    let anchor = graftyard_release::Anchor::from_vars(&install.to_string(), &customer.to_string()).ok()?;
    let b = graftyard_lookup::Bindings { env, sites: &sites };
    graftyard_lookup::view::ensure(&b, &anchor, Some(ctx)).await;
    graftyard_lookup::view::with(|r| (r.count() > 0).then(|| Some(f(&r.fold))))
}

/// `secret`'s active slot (M5-WP6, `Rotate`): from this script's `Release` view when the script binds the members;
/// slot 0 otherwise. `None` when it binds the members but holds no view yet. `MintWrapKey`'s is read only when the
/// minter needs a key, at most once per `SIGNING_KEY_CACHE`; a guessed slot would be cached that long, so the mint
/// answers 503 and the edge retries.
async fn view_slot(env: &Env, ctx: &Context, secret: graftyard_release::SecretName) -> Option<u8> {
    view_fold(env, ctx, |f| f.slot(secret)).await.map(|s| s.unwrap_or(0))
}

#[event(fetch)]
async fn fetch(req: Request, env: Env, ctx: Context) -> Result<Response> {
    if req.method() == Method::Post && req.path().starts_with("/actions/") {
        let path = req.path();
        return actions(req, &env, &ctx, &path).await;
    }
    if req.method() == Method::Post && req.path() == "/wrap" {
        return wrap(req, &env, &ctx).await;
    }
    if req.method() == Method::Post && req.path() == "/unwrap" {
        return unwrap(req, &env, &ctx).await;
    }
    if req.method() != Method::Post || req.path() != "/mint" {
        return Response::error("Not Found", 404);
    }
    let mut res = mint(req, &env, &ctx).await?;
    // The edge counts each mint by the version that answered it (`MINT_ERROR_RATIO`). A version with no
    // `VERSION` binding answers none, and its mints never count against a roll.
    if let Ok(v) = env.get_binding::<WorkerVersionMetadata>("VERSION") {
        let _ = res.headers_mut().set(OPS_VERSION_HEADER, &v.id());
    }
    Ok(res)
}

/// The header naming the version that answered a mint (`workers/edge`, `OPS_VERSION_HEADER`).
const OPS_VERSION_HEADER: &str = "x-graftyard-ops-version";

/// `POST /mint`: one installation token for the edge's `Grant`.
async fn mint(mut req: Request, env: &Env, ctx: &Context) -> Result<Response> {
    // BM-5: a build with `test-mint-fail` is a bad `graft-ops` whose every mint answers 503, so a test install can roll
    // it at 1% and watch the edge's mint errors swap it back.
    if cfg!(feature = "test-mint-fail") {
        return Response::error("test-mint-fail: this test version mints nothing", 503);
    }
    let grant: Grant = match req.json().await {
        Ok(g) => g,
        Err(e) => return Response::error(format!("not a Grant: {e}"), 400),
    };
    let now = Timestamp(Date::now().as_millis());
    // The secret is read only when the minter's key is missing or `SIGNING_KEY_CACHE` old.
    let text = if MINTER.with(|m| m.borrow().needs_key(now)) {
        let Some(slot) = view_slot(env, ctx, graftyard_release::SecretName::MintWrapKey).await else {
            return Response::error("MintWrapKey: no Release view yet", 503);
        };
        match crate::exec::slot_value(env, graftyard_release::SecretName::MintWrapKey, slot).await {
            Ok(t) => t,
            Err(e) => return Response::error(format!("MintWrapKey: {e}"), 503),
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

#[event(scheduled)]
async fn scheduled(_event: ScheduledEvent, env: Env, _ctx: ScheduleContext) {
    // The agents account's `graft-ops` reads the install over HTTPS (`GRAFTYARD_HOST`); the install's has no such
    // variable and reads its own members.
    if env.var("OPS_SCRIPTS").is_err() {
        return;
    }
    if env.var("GRAFTYARD_HOST").is_ok() {
        crate::agents::run(&env).await;
    } else {
        crate::exec::run(&env).await;
    }
}
