//! Clone-cache hit bed (`tests/composed/bed/run.sh`): the edge's hit path in local workerd against an entry larger than
//! the isolate's 128 MB. `/seed?mb=` puts one entry of that many MiB, in 8 MiB parts so seeding never holds it whole;
//! `/hit` answers it as the edge does, through `graftyard_git::host::cached_response`, or with `--features whole` as
//! main 6c9610bf did, read whole with `.bytes()`. `/mem` answers the isolate's wasm memory in MiB, which never shrinks,
//! so after a hit it shows whether the entry passed through wasm: local workerd does not enforce the 128 MB limit that
//! made every hosted hit a 500.
use graftyard_git::host::R2;
use worker::*;

const KEY: &str = "clone/131296/98859749f82dcff16044d975cb63804a1de961a85723402a5fea4fa335914d1c/0";

#[event(fetch)]
async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    let url = req.url()?;
    let b = R2(env.bucket("A")?);
    let h = Headers::new();
    h.set("content-type", "application/x-git-upload-pack-result")?;
    h.set("x-graft-clone-cache", "hit")?;
    match url.path() {
        "/seed" => {
            let mb: usize = url.query_pairs().find(|(k, _)| k == "mb").map_or(300, |(_, v)| v.parse().unwrap());
            let u = b.0.create_multipart_upload(KEY).execute().await?;
            let mut parts = vec![];
            for i in 0..mb.div_ceil(8) {
                let n = (mb - i * 8).min(8) << 20;
                parts.push(u.upload_part(i as u16 + 1, vec![(i % 251) as u8; n]).await?);
            }
            u.complete(parts).await?;
            Response::ok(format!("seeded {mb} MiB\n"))
        }
        #[cfg(not(feature = "whole"))]
        "/hit" => match graftyard_git::host::cached_response(&b, KEY, h).await.map_err(|e| Error::RustError(e.0))? {
            Some(r) => Ok(r),
            None => Response::error("no entry", 404),
        },
        #[cfg(feature = "whole")]
        "/hit" => {
            let o = b.0.get(KEY).execute().await?.ok_or("no entry")?;
            let body = o.body().ok_or("no body")?.bytes().await?;
            Ok(Response::from_bytes(body)?.with_headers(h))
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
