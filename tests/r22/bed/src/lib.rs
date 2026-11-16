//! R22 bed (`tests/r22/bed/run.sh`): crates/git's real `sweep` in local workerd, filling committed packs from bucket A
//! into empty B and C as site a's cron fills network 131296. `/seed?i=&size=` puts one pack of `size` bytes and its
//! marker in A, `/sweep` runs one sweep of the network and reports the wasm heap after it, `/count` checks every copy,
//! and with the `edge` feature (not on main, which has no `copy_new`) `/edge` copies one pack with a wrong SHA-256, a
//! wrong length, then right, then again.
use graftyard_git::host::R2;
use graftyard_git::{
    Bucket, Marker, ObjectMeta, PackId, R2Key, RequestRow, RequestSource, StoreError, put_marker, sweep,
};
use graftyard_types::{ChainId, DbId, Digest, Index, NetworkId, RepoId, RequestId, SiteId, Timestamp, Ulid};
use sha2::{Digest as _, Sha256};
use worker::*;

const NET: NetworkId = NetworkId(DbId(131296));
const CHAIN: ChainId = ChainId::Repo(RepoId(DbId(131296)));

struct NoRows;
impl RequestSource for NoRows {
    async fn request(&self, _c: ChainId, _id: RequestId) -> std::result::Result<Option<RequestRow>, StoreError> {
        Ok(None)
    }
}

#[event(fetch)]
async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    let url = req.url()?;
    let q = |k: &str| url.query_pairs().find(|(n, _)| n == k).map(|(_, v)| v.to_string());
    let b: Vec<(SiteId, R2)> = ["A", "B", "C"]
        .iter()
        .enumerate()
        .map(|(i, n)| Ok((SiteId(i as u8), R2(env.bucket(n)?))))
        .collect::<Result<_>>()?;
    match url.path() {
        "/seed" => {
            let i: u8 = q("i").unwrap().parse().unwrap();
            let size: usize = q("size").unwrap().parse().unwrap();
            let body = vec![i; size];
            let sum = Digest(Sha256::digest(&body).into());
            let id = PackId(Ulid::new(Timestamp(1_790_000_000_000 + i as u64), [i; 10]));
            let key = R2Key::Pack { network: NET, id }.to_string();
            let meta =
                ObjectMeta { chain: CHAIN, request: RequestId(Ulid::new(Timestamp(1), [i; 10])) }.to_pairs().to_vec();
            b[0].1.put_new(&key, &body, sum, &meta).await.map_err(|e| Error::RustError(e.0))?;
            put_marker(
                &b[0].1,
                NET,
                id,
                &Marker { chain: CHAIN, index: Index(i as u64 + 1), checksum: sum, bytes: size as u64 },
            )
            .await
            .map_err(|e| Error::RustError(e.0))?;
            Response::ok("seeded\n")
        }
        "/sweep" => {
            let refs: Vec<(SiteId, &R2)> = b.iter().map(|(s, r)| (*s, r)).collect();
            let t0 = Date::now().as_millis();
            let r = sweep(NET, &refs, &NoRows, Timestamp(Date::now().as_millis()), 600)
                .await
                .map_err(|e| Error::RustError(e.0))?;
            Response::from_json(&serde_json::json!({
                "planned": r.plan.actions.len(), "done": r.done, "failed": r.failed.len(),
                "firstFailure": r.failed.first().map(|(_, e)| e.clone()), "ms": Date::now().as_millis() - t0, "wasmMb": wasm_mb(),
            }))
        }
        #[cfg(feature = "edge")]
        "/edge" => {
            // One seeded pack copied A -> C with a wrong SHA-256, a wrong length, then right, then again (exists).
            let l = b[0].1.list("pack/131296/").await.map_err(|e| Error::RustError(e.0))?;
            let o = &l[0];
            let body = b[0].1.get(&o.key).await.map_err(|e| Error::RustError(e.0))?.unwrap();
            let sum = Digest(Sha256::digest(&body).into());
            let dest = &b[2].1;
            // The sweep filled C already; start from an empty key.
            dest.delete(std::slice::from_ref(&o.key)).await.map_err(|e| Error::RustError(e.0))?;
            let f = |r: std::result::Result<Option<graftyard_git::PutNew>, StoreError>| format!("{r:?}");
            let bad_sum = f(dest.copy_new(&b[0].1, &o.key, Some((o.size, Digest([7; 32])))).await);
            let after_bad_sum = dest.head(&o.key).await.map_err(|e| Error::RustError(e.0))?.is_some();
            let bad_len = f(dest.copy_new(&b[0].1, &o.key, Some((o.size - 1, sum))).await);
            let after_bad_len = dest.head(&o.key).await.map_err(|e| Error::RustError(e.0))?.is_some();
            let ok = f(dest.copy_new(&b[0].1, &o.key, Some((o.size, sum))).await);
            let again = f(dest.copy_new(&b[0].1, &o.key, Some((o.size, sum))).await);
            let missing = f(dest.copy_new(&b[0].1, "pack/131296/nope.idx", None).await);
            let got = dest.get(&o.key).await.map_err(|e| Error::RustError(e.0))?.unwrap();
            Response::from_json(&serde_json::json!({"badSum": bad_sum, "afterBadSum": after_bad_sum, "badLen": bad_len,
                "afterBadLen": after_bad_len, "ok": ok, "again": again, "missing": missing, "same": got == body}))
        }
        "/count" => {
            let mut out = vec![];
            for (_, r) in &b {
                let l = r.list("pack/131296/").await.map_err(|e| Error::RustError(e.0))?;
                let m = r.list("committed/pack/131296/").await.map_err(|e| Error::RustError(e.0))?;
                let mut bad = 0;
                for o in &l {
                    let body = r.get(&o.key).await.map_err(|e| Error::RustError(e.0))?.unwrap();
                    if body.len() != o.size as usize || body.iter().any(|x| *x != body[0]) {
                        bad += 1
                    }
                }
                out.push(serde_json::json!({"packs": l.len(), "markers": m.len(), "bytes": l.iter().map(|o| o.size).sum::<u64>(), "bad": bad,
                    "meta": l.first().map(|o| o.meta.clone())}));
            }
            Response::from_json(&out)
        }
        _ => Response::error("not found", 404),
    }
}

fn wasm_mb() -> f64 {
    use worker::wasm_bindgen::JsCast;
    let m: worker::js_sys::WebAssembly::Memory = worker::wasm_bindgen::memory().unchecked_into();
    let b: worker::js_sys::ArrayBuffer = m.buffer().unchecked_into();
    (b.byte_length() as f64 / 1e5).round() / 10.0
}
