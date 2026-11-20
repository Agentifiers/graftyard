//! `edge-compose` bed (`tests/composed/edgebed/run.sh`): the edge's composed answer in local workerd, built by
//! `graftyard_git::host::manifest_answer` from chain packs and a manifest a real container wrote.
//!
//! - `PUT /put?key=K` stores the body under K in bucket A: the chain's `compose/<net>/<file>` packs, copied from the
//!   container's `/compose-file` as `ready_chain` copies them, and the container's `compose=only&tail=1` answer.
//! - `GET /composed?net=N&key=K` answers the manifest at K as the edge does on a miss or a manifest hit, or 404 when
//!   `manifest_answer` declines it (a missing or changed chain pack, another network's manifest).
//! - `GET /mem` answers the isolate's wasm memory in MiB, which never shrinks, so after an answer it shows whether the
//!   pack passed through wasm memory.
use graftyard_git::host::R2;
use graftyard_types::{DbId, NetworkId};
use worker::*;

#[event(fetch)]
async fn fetch(mut req: Request, env: Env, _ctx: Context) -> Result<Response> {
    let url = req.url()?;
    let q = |k: &str| url.query_pairs().find(|(n, _)| n == k).map(|(_, v)| v.into_owned());
    let b = R2(env.bucket("A")?);
    match url.path() {
        "/put" => {
            let key = q("key").ok_or("key")?;
            let body = req.bytes().await?;
            let n = body.len();
            b.0.put(&key, body).execute().await?;
            Response::ok(format!("{n}\n"))
        }
        "/delete" => {
            b.0.delete(q("key").ok_or("key")?).await?;
            Response::ok("deleted\n")
        }
        "/composed" => {
            let net = NetworkId(DbId(q("net").ok_or("net")?.parse().map_err(|_| "net")?));
            let Some(o) = b.0.get(q("key").ok_or("key")?).execute().await? else {
                return Response::error("no manifest", 404);
            };
            let bytes = o.body().ok_or("no body")?.bytes().await?;
            let h = Headers::new();
            h.set("content-type", "application/x-git-upload-pack-result")?;
            match graftyard_git::host::manifest_answer(&[&b], net, bytes, h).await.map_err(|e| Error::RustError(e.0))? {
                graftyard_git::compose::ChainCheck::Checked(r) => Ok(r),
                graftyard_git::compose::ChainCheck::Declined => Response::error("declined", 404),
                graftyard_git::compose::ChainCheck::Late => Response::error("late", 503),
            }
        }
        "/mem" => {
            use worker::wasm_bindgen::JsCast;
            let m: worker::js_sys::WebAssembly::Memory = worker::wasm_bindgen::memory().unchecked_into();
            let b: worker::js_sys::ArrayBuffer = m.buffer().unchecked_into();
            Response::ok(format!("{}", b.byte_length() >> 20))
        }
        _ => Response::error("not found", 404),
    }
}
