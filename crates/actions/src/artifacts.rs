//! `actions-artifacts` (`design/SHARED.md`, "Actions", Artifacts; `decisions/DECISIONS.md`, "`actions-artifacts`"):
//! legacy pipelines artifacts, the `_apis/pipelines/workflows/{run}/artifacts` surface `upload-artifact@v3.2.2` and
//! `download-artifact@v3` speak through `@actions/artifact` 1.1.3 on a GHES-classified host
//! (`research/actions-protocol.md#40`, `#44`). This module is the pure half: where a run's artifacts live in the owner's
//! home bucket, which job may reach which container, the `Content-Range` a chunk carries, when a file's chunks make
//! the file, and the JSON the client reads. The routes are in `workers/edge/src/artifacts.rs`. Nothing here is fold
//! state: an artifact is R2 objects alone.
//!
//! Objects, all under `actions/artifacts/<chain>/<repo>/<run>/`:
//! - `u/<job>/<item path>`: the upload of that file the job is sending now, a fresh id its first chunk wrote.
//! - `f/<job>/<upload>/<item path>/<start>`: one chunk of one upload, its start offset in 20 digits. A re-upload of a
//!   path, by the same job or another, writes under a new upload, so no upload's chunks ever land among another's.
//! - `i/<item path>`: the file's record (`FileRecord`), written once one upload's chunks tile the file, naming that
//!   upload. The item path starts with the artifact's name, and the last upload to finish is the one a download
//!   reads, whole.
//! - `m/<name>`: the artifact's manifest, written by the client's finalize. Only a name with a manifest is listed.

use graftyard_types::{ChainId, DbId, RepoId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::cache::{Tiling, tiles};
use crate::ids::{JobId, RunId};

pub const ARTIFACTS_ROOT: &str = "actions/artifacts/";
/// The API version `@actions/artifact` 1.1.3 sends (`utils.js`, `getApiVersion`).
pub const API_VERSION: &str = "6.0-preview";
/// An item path's bytes at most, so every key stays under R2's 1,024 [doc: R2 limits, "Object key length"].
pub const ITEM_PATH_MAX: usize = 512;
/// One chunk's bytes at most: the client sends 8 MiB (`config-variables.js`, `getUploadChunkSize`).
pub const CHUNK_MAX: u64 = 64 << 20;

/// Everything one repository's artifacts hold on its owner's shard `chain`: what a repository delete drops.
pub fn repo_prefix(chain: ChainId, repo: RepoId) -> String {
    format!("{ARTIFACTS_ROOT}{chain}/{}/", repo.0.0)
}

/// One run's artifact container, as its `fileContainerResourceUrl` names it: `artifacts/<owner>/<shard>/<repo>/<run>`
/// under the run service.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Container {
    pub chain: ChainId,
    pub repo: RepoId,
    pub run: RunId,
}

impl Container {
    /// The container from the path after `artifacts/`: exactly `<owner>/<shard>/<repo>/<run>`.
    pub fn parse(path: &str) -> Option<Container> {
        let parts: Vec<&str> = path.split('/').collect();
        let [o, s, r, run] = parts.as_slice() else { return None };
        let id = |v: &str| v.parse::<i64>().ok().filter(|n| *n > 0).map(DbId);
        Some(Container {
            chain: ChainId::Actions(graftyard_types::AccountId(id(o)?), s.parse().ok()?),
            repo: RepoId(id(r)?),
            run: RunId(id(run)?),
        })
    }

    pub fn url(&self, services: &str) -> String {
        let ChainId::Actions(owner, shard) = self.chain else { unreachable!("an Actions shard") };
        format!("{services}artifacts/{}/{shard}/{}/{}", owner.0.0, self.repo.0.0, self.run.0.0)
    }

    pub fn prefix(&self) -> String {
        format!("{}{}/", repo_prefix(self.chain, self.repo), self.run.0.0)
    }

    pub fn manifest_key(&self, name: &str) -> String {
        format!("{}m/{name}", self.prefix())
    }

    pub fn manifests(&self) -> String {
        format!("{}m/", self.prefix())
    }

    pub fn record_key(&self, path: &str) -> String {
        format!("{}i/{path}", self.prefix())
    }

    /// Every record of artifact `name`: its item paths all start `<name>/`.
    pub fn records(&self, name: &str) -> String {
        format!("{}i/{name}/", self.prefix())
    }

    /// Where a job's upload of `path` now in progress is named.
    pub fn upload_key(&self, job: JobId, path: &str) -> String {
        format!("{}u/{}/{path}", self.prefix(), job.0.0)
    }

    pub fn chunk_prefix(&self, job: JobId, upload: u64, path: &str) -> String {
        format!("{}f/{}/{upload}/{path}/", self.prefix(), job.0.0)
    }

    pub fn chunk_key(&self, job: JobId, upload: u64, path: &str, start: u64) -> String {
        format!("{}{start:020}", self.chunk_prefix(job, upload, path))
    }
}

/// Why a call is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// The token's job may not reach this container.
    Forbidden,
    NotFound,
    Bad(&'static str),
}

/// Whether the job a token names may reach container `c`: the token's shard, and the job's own run and that run's
/// repository, both read from the shard's fold, so a job reaches only its own run's artifacts and never another
/// repository's prefix, whatever path it sends.
pub fn authorize(c: &Container, token_chain: ChainId, job_run: RunId, run_repo: RepoId) -> Result<(), Refused> {
    if c.chain != token_chain || c.run != job_run || c.repo != run_repo {
        return Err(Refused::Forbidden);
    }
    Ok(())
}

/// The characters the client refuses in a name (`path-and-artifact-name-validation.js`), which this side refuses
/// too, plus the control characters a key should not hold.
fn bad_char(c: char) -> bool {
    matches!(c, '"' | ':' | '<' | '>' | '|' | '*' | '?') || c.is_control()
}

pub fn check_name(name: &str) -> Result<(), Refused> {
    if name.is_empty() || name.len() > 256 || name.contains(['/', '\\']) || name.chars().any(bad_char) {
        return Err(Refused::Bad("the artifact name is not valid"));
    }
    if name == "." || name == ".." {
        return Err(Refused::Bad("the artifact name is not valid"));
    }
    Ok(())
}

/// An item path the client sends as `itemPath`: `<name>/<relative path>`, its name valid, and no segment empty, `.`,
/// or `..`, so no key escapes its container. Answers the artifact's name.
pub fn check_item_path(path: &str) -> Result<&str, Refused> {
    let bad = Refused::Bad("the item path is not valid");
    if path.len() > ITEM_PATH_MAX || path.chars().any(bad_char) || path.contains('\\') {
        return Err(bad);
    }
    let Some((name, rest)) = path.split_once('/') else { return Err(bad) };
    check_name(name)?;
    if rest.split('/').any(|s| s.is_empty() || s == "." || s == "..") {
        return Err(bad);
    }
    Ok(name)
}

/// A chunk's `Content-Range: bytes s-e/total`, inclusive. An empty file is `bytes 0--1/0`, as `getContentRange(0, -1,
/// 0)` writes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chunk {
    Empty,
    Bytes { start: u64, end: u64, total: u64 },
}

pub fn chunk(h: &str) -> Option<Chunk> {
    let (range, total) = h.trim().strip_prefix("bytes ")?.split_once('/')?;
    let total: u64 = total.parse().ok()?;
    if total == 0 {
        return (range == "0--1").then_some(Chunk::Empty);
    }
    let (s, e) = range.split_once('-')?;
    let (start, end): (u64, u64) = (s.parse().ok()?, e.parse().ok()?);
    (start <= end && end < total && end - start < CHUNK_MAX).then_some(Chunk::Bytes { start, end, total })
}

/// A chunk's start offset from its key under `Container::chunk_prefix`. A longer path's chunks under the same prefix
/// (`a/b` and `a/b/c`) are not this file's: their rest is not exactly 20 digits.
pub fn chunk_start(prefix: &str, key: &str) -> Option<u64> {
    let s = key.strip_prefix(prefix)?;
    (s.len() == 20 && s.bytes().all(|b| b.is_ascii_digit())).then(|| s.parse().ok()).flatten()
}

/// One file as its record names it: the job and upload whose chunks hold it, their bytes as stored (`size`), and the
/// file's own length, which differs only when the client gzipped it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileRecord {
    pub job: JobId,
    pub upload: u64,
    pub size: u64,
    pub length: u64,
    pub gzip: bool,
}

/// The record a file gets once `chunks` (start, length) of one upload tile `size`, else why not.
pub fn file_done(
    job: JobId,
    upload: u64,
    chunks: &[(u64, u64)],
    size: u64,
    length: u64,
    gzip: bool,
) -> Result<FileRecord, Tiling> {
    tiles(chunks, size)?;
    Ok(FileRecord { job, upload, size, length, gzip })
}

/// The upload a chunk goes to: a chunk at 0 starts a new one, `fresh`, which a client sends first and alone for each
/// file (`uploadFileAsync` sends one file's chunks in order, and a retry resends the same range); any later chunk goes
/// to the upload its job's pointer names, and with none the chunk is refused, so it cannot join an upload it did not
/// start with.
pub fn upload_for(start: u64, pointer: Option<u64>, fresh: u64) -> Option<u64> {
    if start == 0 { Some(fresh) } else { pointer }
}

/// `ArtifactResponse` (`contracts.d.ts`), for a created or listed artifact.
pub fn artifact_json(c: &Container, services: &str, name: &str, size: i64) -> Value {
    let url = c.url(services);
    json!({
        "containerId": c.run.0.0.to_string(),
        "size": size,
        "signedContent": null,
        "fileContainerResourceUrl": url,
        "type": "actions_storage",
        "name": name,
        "url": format!("{services}run/_apis/pipelines/workflows/{}/artifacts/{name}", c.run.0.0),
    })
}

/// The download URL of one file: the container with `itemPath`, which the client GETs with its bearer token.
pub fn content_location(c: &Container, services: &str, path: &str) -> String {
    let q: String = url_query(path);
    format!("{}?itemPath={q}", c.url(services))
}

fn url_query(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// `ContainerEntry` (`contracts.d.ts`) for one file. The client reads `path`, `itemType`, `fileLength` (0 makes an
/// empty file with no download), and `contentLocation`.
pub fn entry_json(c: &Container, services: &str, path: &str, r: &FileRecord) -> Value {
    json!({
        "containerId": c.run.0.0,
        "scopeIdentifier": "00000000-0000-0000-0000-000000000000",
        "path": path,
        "itemType": "file",
        "status": "created",
        "fileLength": r.length,
        "fileEncoding": 1,
        "fileType": 1,
        "dateCreated": "",
        "dateLastModified": "",
        "createdBy": "",
        "lastModifiedBy": "",
        "itemLocation": content_location(c, services, path),
        "contentLocation": content_location(c, services, path),
        "contentId": "",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use graftyard_types::AccountId;

    const CHAIN: ChainId = ChainId::Actions(AccountId(DbId(7100001)), 11);

    fn c() -> Container {
        Container { chain: CHAIN, repo: RepoId(DbId(65_000)), run: RunId(DbId(41)) }
    }

    #[test]
    fn a_container_url_parses_back_and_nothing_else_does() {
        let url = c().url("https://h/_services/");
        assert_eq!(url, "https://h/_services/artifacts/7100001/11/65000/41");
        assert_eq!(Container::parse("7100001/11/65000/41"), Some(c()));
        for bad in
            ["7100001/11/65000", "7100001/11/65000/41/x", "7100001/x/65000/41", "0/11/65000/41", "7100001/11/-3/41"]
        {
            assert_eq!(Container::parse(bad), None, "{bad}");
        }
    }

    /// The key that isolates runs and repositories is the fold's, not the path's: a job reaches its own run's
    /// container only, and a path naming another repository's prefix, another run, or another shard is refused.
    #[test]
    fn a_job_reaches_only_its_own_runs_container() {
        let own = c();
        assert_eq!(authorize(&own, CHAIN, own.run, own.repo), Ok(()));
        let other_repo = Container { repo: RepoId(DbId(65_001)), ..own };
        assert_eq!(authorize(&other_repo, CHAIN, own.run, own.repo), Err(Refused::Forbidden));
        let other_run = Container { run: RunId(DbId(42)), ..own };
        assert_eq!(authorize(&other_run, CHAIN, own.run, own.repo), Err(Refused::Forbidden));
        let other_shard = Container { chain: ChainId::Actions(AccountId(DbId(7100001)), 12), ..own };
        assert_eq!(authorize(&other_shard, CHAIN, own.run, own.repo), Err(Refused::Forbidden));
        // A repository with the same run number elsewhere: the run alone does not admit it.
        assert_eq!(authorize(&own, CHAIN, own.run, RepoId(DbId(65_001))), Err(Refused::Forbidden));
    }

    #[test]
    fn keys_nest_under_the_repository_prefix_a_delete_drops() {
        let k = c().chunk_key(JobId(DbId(9)), 5, "a/dir/f.bin", 8 << 20);
        assert_eq!(k, "actions/artifacts/actions:7100001.11/65000/41/f/9/5/a/dir/f.bin/00000000000008388608");
        let p = repo_prefix(CHAIN, RepoId(DbId(65_000)));
        for key in [k, c().record_key("a/f"), c().manifest_key("a")] {
            assert!(key.starts_with(&p), "{key}");
        }
        // Repository 6500 is not a prefix of 65000's keys.
        assert!(!c().record_key("a/f").starts_with(&repo_prefix(CHAIN, RepoId(DbId(6500)))));
    }

    #[test]
    fn item_paths_stay_inside_their_artifact() {
        assert_eq!(check_item_path("art/dir/x.txt"), Ok("art"));
        for bad in ["art", "art/", "art//x", "art/../x", "art/./x", "/art/x", "a:b/x", "art\\x", "../x/y"] {
            assert!(check_item_path(bad).is_err(), "{bad}");
        }
        assert!(check_item_path(&format!("art/{}", "x".repeat(ITEM_PATH_MAX))).is_err());
        assert!(check_name("a/b").is_err() && check_name("..").is_err() && check_name("my artifact 1").is_ok());
    }

    #[test]
    fn a_content_range_and_the_empty_files() {
        assert_eq!(
            chunk("bytes 0-8388607/20000000"),
            Some(Chunk::Bytes { start: 0, end: 8_388_607, total: 20_000_000 })
        );
        assert_eq!(chunk("bytes 0--1/0"), Some(Chunk::Empty));
        for bad in ["bytes 5-4/10", "bytes 0-10/10", "bytes 0-1/*", "0-1/2", "bytes 0-0/0"] {
            assert_eq!(chunk(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_longer_paths_chunks_are_not_this_files() {
        let p = c().chunk_prefix(JobId(DbId(9)), 5, "a/b");
        assert_eq!(chunk_start(&p, &format!("{p}00000000000000000005")), Some(5));
        assert_eq!(chunk_start(&p, &format!("{p}c/00000000000000000005")), None);
    }

    /// A file is recorded only when one job's chunks tile it exactly: a missing middle chunk, a stale longer tail, or
    /// a short upload records nothing.
    #[test]
    fn a_file_is_recorded_only_when_its_chunks_tile_it() {
        let j = JobId(DbId(9));
        assert!(file_done(j, 1, &[(0, 8), (8, 4)], 12, 40, true).is_ok());
        assert_eq!(file_done(j, 1, &[(0, 8)], 12, 12, false), Err(Tiling::Short { have: 8 }));
        assert_eq!(file_done(j, 1, &[(0, 4), (8, 4)], 12, 12, false), Err(Tiling::Gap { at: 4 }));
        assert_eq!(file_done(j, 1, &[(0, 8), (8, 8)], 12, 12, false), Err(Tiling::Long { have: 16 }));
    }

    /// A re-upload of one path by the same job starts a new upload at its first chunk, so its chunks never land
    /// among the first upload's: the first stays whole for a reader until the second's record replaces it. A later
    /// chunk with no upload started is refused, not written beside another's.
    #[test]
    fn a_re_upload_never_mixes_with_the_one_before() {
        let (ct, j) = (c(), JobId(DbId(9)));
        let first = upload_for(0, None, 100).unwrap();
        assert_eq!(upload_for(8 << 20, Some(first), 999), Some(first), "a later chunk joins the upload it began");
        let second = upload_for(0, Some(first), 101).unwrap();
        assert_ne!(first, second);
        assert_ne!(ct.chunk_key(j, first, "r/x.bin", 8 << 20), ct.chunk_key(j, second, "r/x.bin", 8 << 20));
        assert!(!ct.chunk_prefix(j, second, "r/x.bin").starts_with(&ct.chunk_prefix(j, first, "r/x.bin")));
        assert_eq!(upload_for(8 << 20, None, 102), None);
    }

    #[test]
    fn the_json_the_client_reads() {
        let s = "https://h/_services/";
        let a = artifact_json(&c(), s, "art", -1);
        assert_eq!(a["fileContainerResourceUrl"], "https://h/_services/artifacts/7100001/11/65000/41");
        let r = FileRecord { job: JobId(DbId(9)), upload: 1, size: 20, length: 300, gzip: true };
        let e = entry_json(&c(), s, "art/a b+c.txt", &r);
        assert_eq!(e["fileLength"], 300, "the file's own length, not the stored gzip's");
        assert_eq!(
            e["contentLocation"],
            "https://h/_services/artifacts/7100001/11/65000/41?itemPath=art/a%20b%2Bc.txt"
        );
    }
}
