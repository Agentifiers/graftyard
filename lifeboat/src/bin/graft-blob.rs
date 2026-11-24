//! `graft-blob`, the lifeboat's object store (`design/DESIGN.md`, "Install operations", Lifeboat): `blob::put`, `get`,
//! `start`, and `scrub` over the host's own filesystem, served on localhost to the bundle's puller and front. A Worker
//! has no `fsync`, so the store runs beside workerd, not in it.
//!
//! - `PUT /<key>` with `x-graftyard-sha256: <hex>`: 400 unless the body hashes to it; 201 once the bytes, the link,
//!   and the directory are durable; 200 for the same bytes again; 409 for other bytes.
//! - `GET /<key>?sha256=<hex>`: the bytes if they hash to it; 404 if absent; 500 `corrupt` and never the bytes if not.
//! - `POST /_scrub`: each key against its recorded SHA-256 (`<key>.sha256` beside it), repaired from `--sibling` roots
//!   that hold a good copy; answers each key's outcome.
//!
//!     graft-blob --root <dir> [--listen 127.0.0.1:8790] [--sibling <dir>]...

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use graftyard_lifeboat::blob::{self, BlobError, Fs, FsError, Scrubbed};

/// The host's filesystem under `root`. `fsync` and `fsync_dir` are real (`File::sync_all` on the file or the open
/// directory), and `link` is a hard link, which fails when the name exists.
struct Disk {
    root: PathBuf,
}

fn io(e: std::io::Error) -> FsError {
    if e.kind() == std::io::ErrorKind::AlreadyExists { FsError::Exists } else { FsError::Io(e.to_string()) }
}

impl Disk {
    fn at(&self, p: &str) -> PathBuf {
        self.root.join(p)
    }
}

impl Fs for Disk {
    fn write(&mut self, path: &str, bytes: &[u8]) -> Result<(), FsError> {
        let p = self.at(path);
        if let Some(d) = p.parent() {
            fs::create_dir_all(d).map_err(io)?;
        }
        let mut f = fs::OpenOptions::new().write(true).create_new(true).open(&p).map_err(io)?;
        f.write_all(bytes).map_err(io)
    }
    fn fsync(&mut self, path: &str) -> Result<(), FsError> {
        fs::File::open(self.at(path)).and_then(|f| f.sync_all()).map_err(io)
    }
    fn link(&mut self, from: &str, to: &str) -> Result<(), FsError> {
        let t = self.at(to);
        if let Some(d) = t.parent() {
            fs::create_dir_all(d).map_err(io)?;
        }
        fs::hard_link(self.at(from), t).map_err(io)
    }
    fn rename(&mut self, from: &str, to: &str) -> Result<(), FsError> {
        fs::rename(self.at(from), self.at(to)).map_err(io)
    }
    fn fsync_dir(&mut self, dir: &str) -> Result<(), FsError> {
        fs::File::open(self.at(dir)).and_then(|f| f.sync_all()).map_err(io)
    }
    fn remove(&mut self, path: &str) -> Result<(), FsError> {
        match fs::remove_file(self.at(path)) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            r => r.map_err(io),
        }
    }
    fn read(&self, path: &str) -> Result<Option<Vec<u8>>, FsError> {
        match fs::read(self.at(path)) {
            Ok(b) => Ok(Some(b)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(io(e)),
        }
    }
    fn list(&self, dir: &str) -> Result<Vec<String>, FsError> {
        match fs::read_dir(self.at(dir)) {
            Ok(rd) => Ok(rd.filter_map(|e| e.ok()?.file_name().into_string().ok()).collect()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(io(e)),
        }
    }
}

fn hex32(s: &str) -> Option<[u8; 32]> {
    (s.len() == 64)
        .then(|| (0..32).map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()).collect::<Option<Vec<u8>>>())??
        .try_into()
        .ok()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// A key is a relative path of `[A-Za-z0-9._-]` segments, none `.` or `..`, not under the temp directory, and not a
/// digest file.
fn key_ok(k: &str) -> bool {
    !k.is_empty()
        && !k.starts_with(blob::TMP)
        && !k.ends_with(".sha256")
        && k.split('/').all(|s| {
            !s.is_empty()
                && s != "."
                && s != ".."
                && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        })
}

/// Every stored key under `dir`, recursively, with its recorded digest file beside it, temp files excluded.
fn keys(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let rel = p.strip_prefix(root).unwrap_or(&p).to_string_lossy().to_string();
        if rel == blob::TMP {
            continue;
        }
        if p.is_dir() {
            keys(root, &p, out);
        } else if !rel.ends_with(".sha256") {
            out.push(rel);
        }
    }
}

fn reply(req: tiny_http::Request, status: u16, body: impl Into<Vec<u8>>) {
    let _ = req.respond(tiny_http::Response::from_data(body.into()).with_status_code(status));
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arg = |k: &str| args.windows(2).find(|w| w[0] == k).map(|w| w[1].clone());
    let root = PathBuf::from(arg("--root").expect("--root <dir>"));
    let listen = arg("--listen").unwrap_or_else(|| "127.0.0.1:8790".into());
    let siblings: Vec<PathBuf> =
        args.windows(2).filter(|w| w[0] == "--sibling").map(|w| PathBuf::from(&w[1])).collect();
    fs::create_dir_all(&root).expect("root");
    let mut disk = Disk { root: root.clone() };
    let left = blob::start(&mut disk).expect("start");
    eprintln!("graft-blob: {} on {listen}; {left} temp files removed", root.display());
    let server = tiny_http::Server::http(&listen).expect("listen");
    let mut nonce: u64 =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
    for mut req in server.incoming_requests() {
        nonce = nonce.wrapping_add(1);
        let url = req.url().to_string();
        let (path, query) = url.split_once('?').unwrap_or((&url, ""));
        let key = path.trim_start_matches('/').to_string();
        match (req.method().as_str(), key.as_str()) {
            ("POST", "_scrub") => {
                let mut all = Vec::new();
                keys(&root, &root, &mut all);
                let mut out = Vec::new();
                for k in all {
                    let want = disk
                        .read(&format!("{k}.sha256"))
                        .ok()
                        .flatten()
                        .and_then(|b| hex32(String::from_utf8_lossy(&b).trim()));
                    let Some(want) = want else { continue };
                    let sib: Vec<Box<dyn Fn() -> Option<Vec<u8>>>> = siblings
                        .iter()
                        .map(|s| {
                            let p = s.join(&k);
                            Box::new(move || fs::read(&p).ok()) as Box<dyn Fn() -> Option<Vec<u8>>>
                        })
                        .collect();
                    let refs: Vec<&dyn Fn() -> Option<Vec<u8>>> = sib.iter().map(|b| b.as_ref()).collect();
                    let got = match blob::scrub(&mut disk, &k, &want, &refs, nonce) {
                        Ok(Scrubbed::Ok) => "ok",
                        Ok(Scrubbed::Repaired) => "repaired",
                        Ok(Scrubbed::Restored) => "restored",
                        Ok(Scrubbed::Lost) => "lost",
                        Err(_) => "error",
                    };
                    out.push(format!("{k} {got}"));
                }
                reply(req, 200, out.join("\n") + "\n");
            }
            (_, k) if !key_ok(k) => reply(req, 400, "bad key\n"),
            ("PUT", k) => {
                let sum =
                    req.headers().iter().find(|h| h.field.equiv("x-graftyard-sha256")).map(|h| h.value.to_string());
                let Some(want) = sum.as_deref().and_then(hex32) else {
                    reply(req, 400, "x-graftyard-sha256 is required\n");
                    continue;
                };
                let mut body = Vec::new();
                if req.as_reader().read_to_end(&mut body).is_err() {
                    reply(req, 400, "body\n");
                    continue;
                }
                if blob::sha256(&body) != want {
                    reply(req, 400, "the body does not hash to x-graftyard-sha256\n");
                    continue;
                }
                let had = disk.read(k).ok().flatten().is_some();
                let r = blob::put(&mut disk, k, &body, nonce).and_then(|()| {
                    blob::put(&mut disk, &format!("{k}.sha256"), hex(&want).as_bytes(), nonce.wrapping_add(1 << 63))
                });
                match r {
                    Ok(()) if had => reply(req, 200, "same\n"),
                    Ok(()) => reply(req, 201, "stored\n"),
                    Err(BlobError::Conflict) => reply(req, 409, "the key holds other bytes\n"),
                    Err(e) => reply(req, 500, format!("{e}\n")),
                }
            }
            ("GET", k) => {
                let want = query.split('&').find_map(|p| p.strip_prefix("sha256=")).and_then(hex32);
                let want = want.or_else(|| {
                    disk.read(&format!("{k}.sha256"))
                        .ok()
                        .flatten()
                        .and_then(|b| hex32(String::from_utf8_lossy(&b).trim()))
                });
                let Some(want) = want else {
                    reply(req, 404, "absent\n");
                    continue;
                };
                match blob::get(&disk, k, &want) {
                    Ok(Some(b)) => reply(req, 200, b),
                    Ok(None) => reply(req, 404, "absent\n"),
                    Err(BlobError::Corrupt) => reply(req, 500, "corrupt\n"),
                    Err(e) => reply(req, 500, format!("{e}\n")),
                }
            }
            _ => reply(req, 405, "method\n"),
        }
    }
}
