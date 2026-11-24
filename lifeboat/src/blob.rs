//! `graft-blob`, the lifeboat's object store on the host's disk (`design/DESIGN.md`, "Install operations", Lifeboat):
//! "writes a temp file, fsyncs it, links it to the final key (failing if the key exists), and fsyncs the directory
//! before any 200. Reads verify `PackRef.checksum`, and a weekly scrub repairs from a sibling copy."
//!
//! The rules are written over `Fs`, the few calls they make, so a test can crash the host between any two of them
//! (`tests/blob.rs`). The bundle's `Fs` is the host's filesystem.
//!
//! - **Put.** Write `tmp/<key>.<nonce>`, fsync it, `link` it to `<key>` (create-only: an existing key is `Exists`),
//!   fsync the key's directory, remove the temp file. Only then `Ok`. A put whose key exists with the same bytes is
//!   `Ok` too, so a retried put after a crash between the link and the answer succeeds; with other bytes it is
//!   `Conflict`, and nothing is overwritten.
//! - **Get.** The bytes, checked against the SHA-256 the caller names (a pack's `PackRef.checksum`); a mismatch is
//!   `Corrupt`, never served.
//! - **Scrub.** Each key against its recorded SHA-256; a corrupt one is replaced from a sibling copy that matches, by
//!   the same temp, fsync, and rename, and one no sibling can repair is reported.
//! - **Start.** Temp files left by a crash are removed; nothing else is touched.

use sha2::{Digest as _, Sha256};

/// The filesystem calls `graft-blob` makes. Paths are relative to the store's root; `/` separates directories.
pub trait Fs {
    fn write(&mut self, path: &str, bytes: &[u8]) -> Result<(), FsError>;
    fn fsync(&mut self, path: &str) -> Result<(), FsError>;
    /// A hard link from `from` to `to`, failing with `Exists` if `to` exists.
    fn link(&mut self, from: &str, to: &str) -> Result<(), FsError>;
    /// Replaces `to` with `from` atomically (the scrub's repair).
    fn rename(&mut self, from: &str, to: &str) -> Result<(), FsError>;
    fn fsync_dir(&mut self, dir: &str) -> Result<(), FsError>;
    fn remove(&mut self, path: &str) -> Result<(), FsError>;
    fn read(&self, path: &str) -> Result<Option<Vec<u8>>, FsError>;
    fn list(&self, dir: &str) -> Result<Vec<String>, FsError>;
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FsError {
    #[error("exists")]
    Exists,
    #[error("io: {0}")]
    Io(String),
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum BlobError {
    #[error("the key holds other bytes")]
    Conflict,
    #[error("the stored bytes do not match their checksum")]
    Corrupt,
    #[error(transparent)]
    Fs(#[from] FsError),
}

pub const TMP: &str = "tmp";

pub fn sha256(b: &[u8]) -> [u8; 32] {
    Sha256::digest(b).into()
}

fn dir_of(key: &str) -> &str {
    key.rsplit_once('/').map_or("", |(d, _)| d)
}

fn tmp_of(key: &str, nonce: u64) -> String {
    format!("{TMP}/{}.{nonce:016x}", key.replace('/', "%"))
}

/// Stores `bytes` under `key`, durably, before it returns `Ok`. `nonce` names the temp file, so two puts of one key
/// never share one.
pub fn put(fs: &mut impl Fs, key: &str, bytes: &[u8], nonce: u64) -> Result<(), BlobError> {
    let tmp = tmp_of(key, nonce);
    fs.write(&tmp, bytes)?;
    fs.fsync(&tmp)?;
    let linked = fs.link(&tmp, key);
    let _ = fs.remove(&tmp);
    match linked {
        Ok(()) => {}
        Err(FsError::Exists) => {
            return match fs.read(key)? {
                Some(have) if have == bytes => {
                    // The earlier put linked it; its directory fsync may not have run.
                    fs.fsync_dir(dir_of(key))?;
                    Ok(())
                }
                _ => Err(BlobError::Conflict),
            };
        }
        Err(e) => return Err(e.into()),
    }
    fs.fsync_dir(dir_of(key))?;
    Ok(())
}

/// The bytes under `key` if they hash to `sha256`; `None` if the key is absent.
pub fn get(fs: &impl Fs, key: &str, want: &[u8; 32]) -> Result<Option<Vec<u8>>, BlobError> {
    match fs.read(key)? {
        None => Ok(None),
        Some(b) if sha256(&b) == *want => Ok(Some(b)),
        Some(_) => Err(BlobError::Corrupt),
    }
}

/// Removes the temp files a crash left. A key is never touched.
pub fn start(fs: &mut impl Fs) -> Result<usize, FsError> {
    let left = fs.list(TMP)?;
    for f in &left {
        fs.remove(&format!("{TMP}/{f}"))?;
    }
    if !left.is_empty() {
        fs.fsync_dir(TMP)?;
    }
    Ok(left.len())
}

/// What one scrub of a key found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scrubbed {
    Ok,
    Repaired,
    /// Corrupt here, and no sibling holds bytes that match.
    Lost,
    /// Absent here; a sibling's copy was put.
    Restored,
}

/// Checks `key` against `want` and repairs it from the first sibling copy that matches.
pub fn scrub(
    fs: &mut impl Fs,
    key: &str,
    want: &[u8; 32],
    siblings: &[&dyn Fn() -> Option<Vec<u8>>],
    nonce: u64,
) -> Result<Scrubbed, BlobError> {
    let have = fs.read(key)?;
    if have.as_deref().is_some_and(|b| sha256(b) == *want) {
        return Ok(Scrubbed::Ok);
    }
    let Some(good) = siblings.iter().find_map(|s| s().filter(|b| sha256(b) == *want)) else {
        return Ok(Scrubbed::Lost);
    };
    if have.is_none() {
        put(fs, key, &good, nonce)?;
        return Ok(Scrubbed::Restored);
    }
    let tmp = tmp_of(key, nonce);
    fs.write(&tmp, &good)?;
    fs.fsync(&tmp)?;
    fs.rename(&tmp, key)?;
    fs.fsync_dir(dir_of(key))?;
    Ok(Scrubbed::Repaired)
}
