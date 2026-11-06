//! The R2 operations the pack store, the sweeper, and rehydrate need, as a trait. `host::R2` implements it over a
//! Workers R2 binding and `mem::MemBucket` in memory. Hosts are single-threaded (Workers isolates), so the futures are
//! not `Send`.
#![allow(async_fn_in_trait)]

use graftyard_types::{Digest, Timestamp};

/// One object as a listing or a HEAD returns it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectInfo {
    pub key: String,
    pub size: u64,
    /// R2's own upload time. The sweeper ages objects by it and by their `PackId` time.
    pub uploaded: Timestamp,
    /// Present for single puts made with a SHA-256; absent for multipart objects.
    pub sha256: Option<Digest>,
    pub meta: Vec<(String, String)>,
}

impl ObjectInfo {
    pub fn meta(&self, key: &str) -> Option<&str> {
        self.meta.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
}

/// A create-only put either created the object or found one there already.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PutNew {
    Created,
    /// The key existed. Its SHA-256, when R2 has one, so a retry of the same bytes can count.
    Exists(Option<Digest>),
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error, serde::Serialize, serde::Deserialize)]
#[error("bucket: {0}")]
pub struct StoreError(pub String);

pub trait Bucket {
    /// `If-None-Match: *` with R2 checking `sha256` against the bytes.
    async fn put_new(
        &self,
        key: &str,
        body: &[u8],
        sha256: Digest,
        meta: &[(String, String)],
    ) -> Result<PutNew, StoreError>;
    /// A create-only put of a large body in `part`-byte multipart parts, its upload id journaled under `journal`
    /// first so the sweeper can abort the upload if the writer dies (`R2Key::Upload`). The key is a fresh `PackId`,
    /// so no other writer holds it. Buckets without multipart put it whole.
    async fn put_new_parts(
        &self,
        key: &str,
        _journal: &str,
        body: &[u8],
        sha256: Digest,
        meta: &[(String, String)],
        _part: usize,
    ) -> Result<PutNew, StoreError> {
        self.put_new(key, body, sha256, meta).await
    }
    async fn head(&self, key: &str) -> Result<Option<ObjectInfo>, StoreError>;
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, StoreError>;
    /// Every object under `prefix`, all pages, with custom metadata.
    async fn list(&self, prefix: &str) -> Result<Vec<ObjectInfo>, StoreError>;
    /// The distinct next path segments under `prefix` (`pack/` gives each network id), as R2's delimited listing
    /// returns them. The default lists every key; `host::R2` asks R2 for the delimited prefixes instead.
    async fn segments(&self, prefix: &str) -> Result<Vec<String>, StoreError> {
        let mut out: Vec<String> = self
            .list(prefix)
            .await?
            .into_iter()
            .filter_map(|o| o.key.strip_prefix(prefix).and_then(|r| r.split('/').next()).map(str::to_owned))
            .collect();
        out.sort();
        out.dedup();
        Ok(out)
    }
    async fn delete(&self, keys: &[String]) -> Result<(), StoreError>;
    /// Aborts the multipart upload `upload_id` of `key`. An upload that no longer exists, completed or aborted, is not
    /// an error.
    async fn abort_upload(&self, key: &str, upload_id: &str) -> Result<(), StoreError>;
}
