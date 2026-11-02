//! An in-memory `Bucket` with a settable clock and injected failures, for tests in this crate and in its hosts.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;

use graftyard_types::{Digest, Timestamp};
use sha2::{Digest as _, Sha256};

use crate::bucket::{Bucket, ObjectInfo, PutNew, StoreError};

#[derive(Default)]
pub struct MemBucket {
    objects: RefCell<BTreeMap<String, (Vec<u8>, ObjectInfo)>>,
    /// Open multipart uploads: (key, upload id).
    uploads: RefCell<Vec<(String, String)>>,
    now: Cell<u64>,
    fail_puts: Cell<bool>,
    fail_prefix: RefCell<Option<String>>,
}

impl MemBucket {
    pub fn set_now(&self, at: Timestamp) {
        self.now.set(at.0);
    }

    pub fn fail_puts(&self, on: bool) {
        self.fail_puts.set(on);
    }

    /// Fails only puts whose key starts with `prefix`, such as markers.
    pub fn fail_puts_under(&self, prefix: Option<&str>) {
        *self.fail_prefix.borrow_mut() = prefix.map(str::to_owned);
    }

    pub fn keys(&self) -> Vec<String> {
        self.objects.borrow().keys().cloned().collect()
    }

    /// Puts with no checks, at an explicit upload time, as a test plants state.
    pub fn plant(&self, key: &str, body: &[u8], uploaded: Timestamp, meta: &[(String, String)]) {
        let info = ObjectInfo {
            key: key.to_owned(),
            size: body.len() as u64,
            uploaded,
            sha256: Some(Digest(Sha256::digest(body).into())),
            meta: meta.to_vec(),
        };
        self.objects.borrow_mut().insert(key.to_owned(), (body.to_vec(), info));
    }

    pub fn open_upload(&self, key: &str, upload_id: &str) {
        self.uploads.borrow_mut().push((key.to_owned(), upload_id.to_owned()));
    }

    pub fn open_uploads(&self) -> Vec<(String, String)> {
        self.uploads.borrow().clone()
    }
}

impl Bucket for MemBucket {
    async fn put_new(
        &self,
        key: &str,
        body: &[u8],
        sha256: Digest,
        meta: &[(String, String)],
    ) -> Result<PutNew, StoreError> {
        let failing = self.fail_puts.get() || self.fail_prefix.borrow().as_deref().is_some_and(|p| key.starts_with(p));
        if failing {
            return Err(StoreError("injected put failure".into()));
        }
        if Digest(Sha256::digest(body).into()) != sha256 {
            return Err(StoreError("sha256 mismatch".into()));
        }
        if let Some((_, info)) = self.objects.borrow().get(key) {
            return Ok(PutNew::Exists(info.sha256));
        }
        self.plant(key, body, Timestamp(self.now.get()), meta);
        Ok(PutNew::Created)
    }

    async fn head(&self, key: &str) -> Result<Option<ObjectInfo>, StoreError> {
        Ok(self.objects.borrow().get(key).map(|(_, i)| i.clone()))
    }

    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, StoreError> {
        Ok(self.objects.borrow().get(key).map(|(b, _)| b.clone()))
    }

    async fn list(&self, prefix: &str) -> Result<Vec<ObjectInfo>, StoreError> {
        Ok(self
            .objects
            .borrow()
            .range(prefix.to_owned()..)
            .take_while(|(k, _)| k.starts_with(prefix))
            .map(|(_, (_, i))| i.clone())
            .collect())
    }

    async fn delete(&self, keys: &[String]) -> Result<(), StoreError> {
        let mut o = self.objects.borrow_mut();
        for k in keys {
            o.remove(k);
        }
        Ok(())
    }

    async fn abort_upload(&self, key: &str, upload_id: &str) -> Result<(), StoreError> {
        self.uploads.borrow_mut().retain(|(k, u)| !(k == key && u == upload_id));
        Ok(())
    }
}
