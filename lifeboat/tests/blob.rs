//! `graft-blob` against a filesystem that can crash between any two calls: whatever a crash loses, no `put` that
//! returned `Ok` loses its bytes, no key ever holds bytes that were not put whole, and a retried put succeeds.

use std::collections::BTreeMap;

use graftyard_lifeboat::blob::{self, BlobError, Fs, FsError, Scrubbed, sha256};

/// A disk with a page cache. File contents are durable after `fsync`; a name (a link, a rename, a removal) is durable
/// after `fsync_dir` of its directory, and may also reach the disk before it, as a journaling filesystem may commit
/// it at any time. A crash keeps what is durable and, in one of its two outcomes, every name too. `crash_at` crashes the call with that
/// number, before it runs, and every call after it fails.
#[derive(Clone, Default)]
struct Disk {
    /// Inode contents: volatile, and durable.
    data: BTreeMap<u64, (Vec<u8>, Option<Vec<u8>>)>,
    /// Names: volatile, and per directory the durable set.
    names: BTreeMap<String, u64>,
    durable_names: BTreeMap<String, u64>,
    next: u64,
    calls: usize,
    crash_at: Option<usize>,
    down: bool,
}

fn dir(p: &str) -> &str {
    p.rsplit_once('/').map_or("", |(d, _)| d)
}

impl Disk {
    fn call(&mut self) -> Result<(), FsError> {
        self.calls += 1;
        if self.down || self.crash_at == Some(self.calls) {
            self.down = true;
            return Err(FsError::Io("crashed".into()));
        }
        Ok(())
    }

    /// The disk after a power loss: the durable names, or with `names` every name, each with its inode's durable
    /// contents (an inode never fsynced reads as empty, as a newly created file can).
    fn after_crash_with(&self, names: bool) -> Disk {
        let mut d = Disk { next: self.next, ..Default::default() };
        for (n, ino) in if names { &self.names } else { &self.durable_names } {
            let bytes = self.data.get(ino).and_then(|(_, durable)| durable.clone()).unwrap_or_default();
            d.data.insert(*ino, (bytes.clone(), Some(bytes)));
            d.names.insert(n.clone(), *ino);
            d.durable_names.insert(n.clone(), *ino);
        }
        d
    }

    fn after_crash(&self) -> Disk {
        self.after_crash_with(false)
    }
}

impl Fs for Disk {
    fn write(&mut self, path: &str, bytes: &[u8]) -> Result<(), FsError> {
        self.call()?;
        self.next += 1;
        self.data.insert(self.next, (bytes.to_vec(), None));
        self.names.insert(path.into(), self.next);
        Ok(())
    }
    fn fsync(&mut self, path: &str) -> Result<(), FsError> {
        self.call()?;
        let ino = self.names[path];
        let d = self.data.get_mut(&ino).unwrap();
        d.1 = Some(d.0.clone());
        Ok(())
    }
    fn link(&mut self, from: &str, to: &str) -> Result<(), FsError> {
        self.call()?;
        if self.names.contains_key(to) {
            return Err(FsError::Exists);
        }
        let ino = self.names[from];
        self.names.insert(to.into(), ino);
        Ok(())
    }
    fn rename(&mut self, from: &str, to: &str) -> Result<(), FsError> {
        self.call()?;
        let ino = self.names.remove(from).unwrap();
        self.names.insert(to.into(), ino);
        Ok(())
    }
    fn fsync_dir(&mut self, d: &str) -> Result<(), FsError> {
        self.call()?;
        self.durable_names.retain(|n, _| dir(n) != d);
        for (n, ino) in &self.names {
            if dir(n) == d {
                self.durable_names.insert(n.clone(), *ino);
            }
        }
        Ok(())
    }
    fn remove(&mut self, path: &str) -> Result<(), FsError> {
        self.call()?;
        self.names.remove(path);
        Ok(())
    }
    fn read(&self, path: &str) -> Result<Option<Vec<u8>>, FsError> {
        Ok(self.names.get(path).map(|ino| self.data[ino].0.clone()))
    }
    fn list(&self, d: &str) -> Result<Vec<String>, FsError> {
        Ok(self.names.keys().filter(|n| dir(n) == d).map(|n| n.rsplit_once('/').unwrap().1.to_string()).collect())
    }
}

const KEY: &str = "pack/5/0190aa";

/// A crash before each call of a put, in turn: the put never returns `Ok` without its bytes surviving, the key never
/// holds anything but the whole bytes, and once the host restarts a retried put of the same bytes succeeds.
#[test]
fn no_crash_point_leaves_an_ok_without_durable_bytes() {
    let bytes = b"PACK\x00\x00\x00\x02 the pack body".to_vec();
    let calls = {
        let mut d = Disk::default();
        blob::put(&mut d, KEY, &bytes, 1).unwrap();
        d.calls
    };
    assert!(calls >= 5);
    for (at, names) in (1..=calls + 1).flat_map(|at| [(at, false), (at, true)]) {
        let mut d = Disk { crash_at: Some(at), ..Default::default() };
        let r = blob::put(&mut d, KEY, &bytes, 1);
        let mut after = d.after_crash_with(names);
        let got = after.read(KEY).unwrap();
        if r.is_ok() {
            assert_eq!(got.as_deref(), Some(&bytes[..]), "crash at {at}: an Ok put lost its bytes");
        }
        assert!(got.is_none() || got.as_deref() == Some(&bytes[..]), "crash at {at}: the key holds partial bytes");
        blob::start(&mut after).unwrap();
        assert!(after.list(blob::TMP).unwrap().is_empty(), "crash at {at}: a temp file survived start");
        blob::put(&mut after, KEY, &bytes, 2).unwrap();
        let fin = after.after_crash();
        assert_eq!(fin.read(KEY).unwrap().as_deref(), Some(&bytes[..]), "crash at {at}: the retry is not durable");
    }
}

#[test]
fn a_key_is_never_overwritten_and_reads_are_checked() {
    let mut d = Disk::default();
    blob::put(&mut d, KEY, b"one", 1).unwrap();
    blob::put(&mut d, KEY, b"one", 2).unwrap();
    assert_eq!(blob::put(&mut d, KEY, b"two", 3), Err(BlobError::Conflict));
    assert_eq!(blob::get(&d, KEY, &sha256(b"one")).unwrap().as_deref(), Some(&b"one"[..]));
    assert_eq!(blob::get(&d, KEY, &sha256(b"two")), Err(BlobError::Corrupt));
    assert_eq!(blob::get(&d, "pack/5/other", &sha256(b"one")), Ok(None));
}

#[test]
fn the_scrub_repairs_from_a_sibling_that_matches_and_reports_what_none_can() {
    let mut d = Disk::default();
    blob::put(&mut d, KEY, b"good", 1).unwrap();
    let want = sha256(b"good");
    assert_eq!(blob::scrub(&mut d, KEY, &want, &[], 2), Ok(Scrubbed::Ok));
    // Bit rot under the key.
    let ino = d.names[KEY];
    d.data.get_mut(&ino).unwrap().0 = b"gbod".to_vec();
    let bad = || Some(b"also bad".to_vec());
    let good = || Some(b"good".to_vec());
    assert_eq!(blob::scrub(&mut d, KEY, &want, &[&bad], 3), Ok(Scrubbed::Lost));
    assert_eq!(blob::scrub(&mut d, KEY, &want, &[&bad, &good], 4), Ok(Scrubbed::Repaired));
    assert_eq!(d.after_crash().read(KEY).unwrap().as_deref(), Some(&b"good"[..]), "the repair is durable");
    assert_eq!(blob::scrub(&mut d, "pack/5/gone", &want, &[&good], 5), Ok(Scrubbed::Restored));
}
