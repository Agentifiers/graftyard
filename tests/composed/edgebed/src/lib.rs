//! `edge-compose` bed (`tests/composed/edgebed/run.sh`): the edge's composed answer in local workerd, built by
//! `graftyard_git::host::manifest_answer` from chain packs and a manifest a real container wrote.
//!
//! - `PUT /put?key=K` stores the body under K in bucket A: the chain's `compose/<net>/<file>` packs, copied from the
//!   container's `/compose-file` as `ready_chain` copies them, and the container's `compose=only&tail=1` answer.
//! - `GET /composed?net=N&key=K` answers the manifest at K as the edge does on a miss or a manifest hit, or 404 when
//!   `manifest_answer` declines it (a missing or changed chain pack, another network's manifest).
//! - `GET /mem` answers the isolate's wasm memory in MiB, which never shrinks, so after an answer it shows whether the
//!   pack passed through wasm memory.
//! - `GET /peak` answers the most bytes the isolate's allocator held at once since the last `/peak`, then starts over,
//!   so a run reads one request's peak buffered bytes (`hit-gate`: a tail read whole and copied held twice its size).
use graftyard_git::host::R2;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

/// The system allocator, counting the bytes it holds now and the most it held since `/peak` last read it.
struct Counting;
static NOW: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
unsafe impl std::alloc::GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: std::alloc::Layout) -> *mut u8 {
        let n = NOW.fetch_add(l.size(), Relaxed) + l.size();
        PEAK.fetch_max(n, Relaxed);
        unsafe { std::alloc::System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: std::alloc::Layout) {
        NOW.fetch_sub(l.size(), Relaxed);
        unsafe { std::alloc::System.dealloc(p, l) }
    }
}
#[global_allocator]
static A: Counting = Counting;
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
            let size = o.size();
            let ResponseBody::Stream(body) = o.body().ok_or("no body")?.response_body()? else {
                return Response::error("no stream", 500);
            };
            let h = Headers::new();
            h.set("content-type", "application/x-git-upload-pack-result")?;
            match graftyard_git::host::manifest_answer(&[&b], net, body, size, h).await.map_err(|e| Error::RustError(e.0))? {
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
        "/peak" => {
            let now = NOW.load(Relaxed);
            Response::ok(format!("{} {now}", PEAK.swap(now, Relaxed)))
        }
        _ => Response::error("not found", 404),
    }
}
