//! An in-memory `Bucket` with a settable clock and injected failures, for tests in this crate and in its hosts.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;

use graftyard_types::{Digest, Timestamp};
use sha2::{Digest as _, Sha256};

use crate::bucket::{Bucket, Multipart, ObjectInfo, PartBuf, PutNew, StoreError};

#[derive(Default)]
pub struct MemBucket {
    objects: RefCell<BTreeMap<String, (Vec<u8>, ObjectInfo)>>,
    /// Open multipart uploads: (key, upload id).
    uploads: RefCell<Vec<(String, String)>>,
    /// The parts put to open uploads: (key, upload id, part number) to its bytes.
    parts: RefCell<BTreeMap<(String, String, u16), Vec<u8>>>,
    fail_parts_from: Cell<Option<u16>>,
    now: Cell<u64>,
    fail_puts: Cell<bool>,
    fail_gets: Cell<bool>,
    fail_prefix: RefCell<Option<String>>,
    /// Bytes `get` has handed to callers: on a Workers host, bytes in the isolate's memory.
    read: Cell<u64>,
}

impl MemBucket {
    pub fn set_now(&self, at: Timestamp) {
        self.now.set(at.0);
    }

    pub fn fail_puts(&self, on: bool) {
        self.fail_puts.set(on);
    }

    /// Makes every `get`, `head`, and `list` fail, as an unreadable bucket does.
    pub fn fail_gets(&self, on: bool) {
        self.fail_gets.set(on);
    }

    fn readable(&self) -> Result<(), StoreError> {
        if self.fail_gets.get() { Err(StoreError("injected read failure".into())) } else { Ok(()) }
    }

    /// Fails only puts whose key starts with `prefix`, such as markers.
    pub fn fail_puts_under(&self, prefix: Option<&str>) {
        *self.fail_prefix.borrow_mut() = prefix.map(str::to_owned);
    }

    /// The bytes `get` has returned so far.
    pub fn bytes_read(&self) -> u64 {
        self.read.get()
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
        self.put_failing(key)?;
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
        self.readable()?;
        Ok(self.objects.borrow().get(key).map(|(_, i)| i.clone()))
    }

    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, StoreError> {
        self.readable()?;
        let b = self.objects.borrow().get(key).map(|(b, _)| b.clone());
        self.read.set(self.read.get() + b.as_ref().map_or(0, |b| b.len() as u64));
        Ok(b)
    }

    async fn copy_new(
        &self,
        from: &Self,
        key: &str,
        expect: Option<(u64, Digest)>,
    ) -> Result<Option<PutNew>, StoreError> {
        from.readable()?;
        let Some((body, info)) = from.objects.borrow().get(key).cloned() else { return Ok(None) };
        if let Some((n, sum)) = expect
            && (body.len() as u64 != n || Digest(Sha256::digest(&body).into()) != sum)
        {
            return Err(StoreError(format!("{key}: the copy's length or SHA-256 is not the expected one")));
        }
        self.put_failing(key)?;
        if let Some((_, i)) = self.objects.borrow().get(key) {
            return Ok(Some(PutNew::Exists(i.sha256)));
        }
        self.plant(key, &body, Timestamp(self.now.get()), &info.meta);
        Ok(Some(PutNew::Created))
    }

    async fn list(&self, prefix: &str) -> Result<Vec<ObjectInfo>, StoreError> {
        self.readable()?;
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
        self.parts.borrow_mut().retain(|(k, u, _), _| !(k == key && u == upload_id));
        Ok(())
    }
}

thread_local! {
    /// `MemPart`s alive on this thread now, and the most alive at once since `MemPart::reset_peak`.
    static PARTS: Cell<(usize, usize)> = const { Cell::new((0, 0)) };
}

/// `MemBucket`'s part buffer, counted while it lives, so a test reads how many parts a streamed tee held at once.
pub struct MemPart(Vec<u8>);

impl MemPart {
    /// The most `MemPart`s alive at once on this thread since the last call.
    pub fn reset_peak() -> usize {
        PARTS.with(|p| {
            let (now, peak) = p.get();
            p.set((now, now));
            peak
        })
    }
}

impl Drop for MemPart {
    fn drop(&mut self) {
        PARTS.with(|p| p.set((p.get().0 - 1, p.get().1)));
    }
}

impl PartBuf for MemPart {
    fn with_capacity(n: usize) -> Self {
        PARTS.with(|p| {
            let (now, peak) = p.get();
            p.set((now + 1, peak.max(now + 1)));
        });
        MemPart(Vec::with_capacity(n))
    }
    fn push(&mut self, b: &[u8]) {
        self.0.extend_from_slice(b);
    }
    fn filled(&self) -> usize {
        self.0.len()
    }
    fn clear(&mut self) {
        self.0.clear();
    }
}

impl MemBucket {
    fn put_failing(&self, key: &str) -> Result<(), StoreError> {
        let failing = self.fail_puts.get() || self.fail_prefix.borrow().as_deref().is_some_and(|p| key.starts_with(p));
        if failing { Err(StoreError("injected put failure".into())) } else { Ok(()) }
    }

    /// Fails every part put from part `n` on, as a bucket that goes down mid-pack.
    pub fn fail_parts_from(&self, n: u16) {
        self.fail_parts_from.set(Some(n));
    }
}

impl Multipart for MemBucket {
    type Part = MemPart;

    async fn put_whole(
        &self,
        key: &str,
        part: &MemPart,
        sha256: Digest,
        meta: &[(String, String)],
    ) -> Result<PutNew, StoreError> {
        self.put_new(key, &part.0, sha256, meta).await
    }

    async fn start_upload(
        &self,
        key: &str,
        journal: &str,
        meta: &[(String, String)],
    ) -> Result<Result<String, PutNew>, StoreError> {
        self.put_failing(key)?;
        if let Some((_, info)) = self.objects.borrow().get(key) {
            return Ok(Err(PutNew::Exists(info.sha256)));
        }
        let id = format!("u{}", self.uploads.borrow().len() + self.parts.borrow().len() + 1);
        self.open_upload(key, &id);
        let mut jm = meta.to_vec();
        jm.push(("upload".into(), id.clone()));
        self.plant(journal, b"", Timestamp(self.now.get()), &jm);
        Ok(Ok(id))
    }

    async fn put_part(&self, key: &str, upload_id: &str, n: u16, part: &MemPart) -> Result<String, StoreError> {
        self.put_failing(key)?;
        if self.fail_parts_from.get().is_some_and(|f| n >= f) {
            return Err(StoreError(format!("injected failure of part {n}")));
        }
        if !self.uploads.borrow().iter().any(|(k, u)| k == key && u == upload_id) {
            return Err(StoreError("NoSuchUpload".into()));
        }
        self.parts.borrow_mut().insert((key.to_owned(), upload_id.to_owned(), n), part.0.clone());
        Ok(format!("e{n}"))
    }

    async fn complete_upload(
        &self,
        key: &str,
        upload_id: &str,
        journal: &str,
        parts: &[(u16, String)],
    ) -> Result<(), StoreError> {
        let mut body = Vec::new();
        for (n, etag) in parts {
            let p = self.parts.borrow_mut().remove(&(key.to_owned(), upload_id.to_owned(), *n));
            match p {
                Some(b) if *etag == format!("e{n}") => body.extend_from_slice(&b),
                _ => return Err(StoreError(format!("part {n} was not put"))),
            }
        }
        self.uploads.borrow_mut().retain(|(k, u)| !(k == key && u == upload_id));
        let meta = self.objects.borrow().get(journal).map(|(_, i)| i.meta.clone()).unwrap_or_default();
        let meta: Vec<_> = meta.into_iter().filter(|(k, _)| k != "upload").collect();
        self.plant(key, &body, Timestamp(self.now.get()), &meta);
        // A multipart object carries no SHA-256 of the whole object (`ObjectInfo::sha256`).
        if let Some((_, info)) = self.objects.borrow_mut().get_mut(key) {
            info.sha256 = None;
        }
        self.delete(&[journal.to_owned()]).await
    }
}
