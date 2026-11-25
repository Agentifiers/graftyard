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

use graftyard_types::{ChainId, DbId, RepoId, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::cache::{Tiling, tiles};
use crate::ids::{JobId, RunId};

pub const ARTIFACTS_ROOT: &str = "actions/artifacts/";
/// The API version `@actions/artifact` 1.1.3 sends (`utils.js`, `getApiVersion`).
pub const API_VERSION: &str = "6.0-preview";
/// An item path's bytes at most, so every key stays under R2's 1,024 [doc: R2 limits, "Object key length"].
pub const ITEM_PATH_MAX: usize = 512;
/// One chunk's bytes at most: the client sends 8 MiB (`config-variables.js`, `getUploadChunkSize`).
pub const CHUNK_MAX: u64 = 64 << 20;
/// The default and maximum artifact retention, in days.
pub const RETENTION_DAYS_MAX: u64 = 90;
/// How long a zip URL works: "This URL expires after 1 minute" [doc: GHES 3.22 REST, "Download an artifact"].
pub const ZIP_URL_LIFE_S: u64 = 60;
const DAY_MS: u64 = 86_400_000;

/// Everything one repository's artifacts hold on its owner's shard `chain`: what a repository delete drops.
pub fn repo_prefix(chain: ChainId, repo: RepoId) -> String {
    format!("{ARTIFACTS_ROOT}{chain}/{}/", repo.0.0)
}

/// One finalized artifact's row in the repository index.
pub fn index_key(chain: ChainId, repo: RepoId, id: i64) -> String {
    format!("{}a/{id}", repo_prefix(chain, repo))
}

/// Every finalized artifact's row in the repository index.
pub fn index_prefix(chain: ChainId, repo: RepoId) -> String {
    format!("{}a/", repo_prefix(chain, repo))
}

/// The retention a create request grants: the default unless asked for at least one day, capped at the maximum.
pub fn retention_days(asked: Option<i64>) -> u64 {
    match asked {
        Some(days) if days >= 1 => (days as u64).min(RETENTION_DAYS_MAX),
        _ => RETENTION_DAYS_MAX,
    }
}

/// A stable, positive JavaScript-safe id for one artifact name in one run.
pub fn artifact_id(chain: ChainId, repo: RepoId, run: RunId, name: &str) -> i64 {
    let hash = Sha256::digest(format!("graftyard/artifact/1\0{chain}\0{}\0{}\0{name}", repo.0.0, run.0.0).as_bytes());
    let id = u64::from_be_bytes(hash[..8].try_into().expect("eight hash bytes")) & ((1 << 53) - 1);
    if id == 0 { 1 } else { id as i64 }
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

    /// Where the create POST writes the artifact's retention and creation time.
    pub fn create_key(&self, name: &str) -> String {
        format!("{}c/{name}", self.prefix())
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

/// One finalized artifact's REST metadata, with millisecond timestamps.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactRow {
    pub id: i64,
    pub name: String,
    pub size: i64,
    pub run: RunId,
    pub created: u64,
    pub updated: u64,
    pub expires: u64,
    pub head_sha: String,
    pub head_branch: Option<String>,
}

impl ArtifactRow {
    pub fn expired(&self, now_ms: u64) -> bool {
        now_ms >= self.expires
    }

    pub fn expires_at(created_ms: u64, days: u64) -> u64 {
        created_ms + days * DAY_MS
    }
}

/// What the create POST keeps for the finalize: the retention the client asked for (`RetentionDays`, sent only when
/// the workflow sets `retention-days`) and when the artifact was created, in ms.
pub fn create_record(asked: Option<i64>, now_ms: u64) -> Value {
    json!({ "days": retention_days(asked), "created": now_ms })
}

/// The row a finalize writes: its creation and retention from the create POST's record (90 days from now without
/// one), its size as the client finalized it, and the run's head.
pub fn finalized_row(
    ct: &Container,
    name: &str,
    size: i64,
    created: Option<&Value>,
    run: &crate::shard::WorkflowRun,
    now_ms: u64,
) -> ArtifactRow {
    let at = created.and_then(|v| v["created"].as_u64()).unwrap_or(now_ms);
    let days = created.and_then(|v| v["days"].as_u64()).unwrap_or(RETENTION_DAYS_MAX).min(RETENTION_DAYS_MAX);
    ArtifactRow {
        id: artifact_id(ct.chain, ct.repo, ct.run, name),
        name: name.to_owned(),
        size,
        run: ct.run,
        created: at,
        updated: now_ms,
        expires: ArtifactRow::expires_at(at, days),
        head_sha: run.head_sha.iter().map(|b| format!("{b:02x}")).collect(),
        head_branch: run.head_ref.strip_prefix("refs/heads/").map(str::to_owned),
    }
}

/// The repository and API base an artifact's REST JSON names.
pub struct RestCtx<'a> {
    pub api: &'a str,
    pub full_name: &'a str,
    pub repo: RepoId,
}

/// GHES's `artifact` schema for one row at `now_ms`.
pub fn rest_json(c: &RestCtx<'_>, a: &ArtifactRow, now_ms: u64) -> Value {
    let url = format!("{}/repos/{}/actions/artifacts/{}", c.api, c.full_name, a.id);
    json!({
        "id": a.id,
        "node_id": graftyard_wire::ids::legacy("Artifact", a.id),
        "name": a.name,
        "size_in_bytes": a.size,
        "url": url,
        "archive_download_url": format!("{url}/zip"),
        "expired": a.expired(now_ms),
        "created_at": graftyard_wire::time::iso(Timestamp(a.created)),
        "expires_at": graftyard_wire::time::iso(Timestamp(a.expires)),
        "updated_at": graftyard_wire::time::iso(Timestamp(a.updated)),
        "workflow_run": {
            "id": a.run.0.0,
            "repository_id": c.repo.0.0,
            "head_repository_id": c.repo.0.0,
            "head_branch": a.head_branch,
            "head_sha": a.head_sha,
        },
    })
}

/// Filter and newest-first sort artifact rows, then take one page.
pub fn page_rows(
    rows: Vec<ArtifactRow>,
    name: Option<&str>,
    run: Option<RunId>,
    page: graftyard_wire::Page,
) -> (usize, Vec<ArtifactRow>, bool) {
    let mut rows: Vec<ArtifactRow> =
        rows.into_iter().filter(|r| name.is_none_or(|n| r.name == n) && run.is_none_or(|run| r.run == run)).collect();
    rows.sort_by(|a, b| b.created.cmp(&a.created).then_with(|| b.id.cmp(&a.id)));
    let total = rows.len();
    let page_rows = rows.into_iter().skip(page.skip()).take(page.per_page).collect();
    (total, page_rows, total > page.end())
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

/// The artifact name a key relative to its run prefix belongs to.
pub fn of_artifact(rel: &str) -> Option<&str> {
    fn named(rest: &str) -> Option<&str> {
        let (name, tail) = rest.split_once('/')?;
        (!name.is_empty() && !tail.is_empty()).then_some(name)
    }

    match rel.split_once('/') {
        Some(("m" | "c", name)) => (!name.is_empty() && !name.contains('/')).then_some(name),
        Some(("i", rest)) => named(rest),
        Some(("u", rest)) => {
            let (job, rest) = rest.split_once('/')?;
            if job.is_empty() { None } else { named(rest) }
        }
        Some(("f", rest)) => {
            let (job, rest) = rest.split_once('/')?;
            let (upload, rest) = rest.split_once('/')?;
            if job.is_empty() || upload.is_empty() { None } else { named(rest) }
        }
        _ => None,
    }
}

/// A file's path inside its artifact's zip.
pub fn zip_path<'p>(path: &'p str, name: &str) -> Option<&'p str> {
    path.strip_prefix(format!("{name}/").as_str())
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

/// Where the zip URL's ticket for artifact `id` lives: one object per `GET .../zip`, named by 128 random bits and
/// holding the artifact's id and the ticket's expiry, so the URL needs no key and the repository delete drops it.
pub fn ticket_key(chain: ChainId, repo: RepoId, nonce: &str) -> String {
    format!("{}z/{nonce}", repo_prefix(chain, repo))
}

/// A ticket's name: 32 lowercase hex digits.
pub fn ticket_name(bytes: &[u8; 16]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Whether a ticket `{"id", "exp"}` (seconds) admits artifact `id` at `now`.
pub fn ticket_admits(ticket: &Value, id: i64, now: Timestamp) -> bool {
    ticket["id"].as_i64() == Some(id) && ticket["exp"].as_u64().is_some_and(|e| e > now.0 / 1000)
}

/// The run service URL of one artifact's zip, named by its ticket.
pub fn zip_url(services: &str, chain: ChainId, repo: RepoId, id: i64, nonce: &str) -> String {
    let ChainId::Actions(owner, shard) = chain else { unreachable!("an Actions shard") };
    format!("{services}artifact-zip/{}/{shard}/{}/{id}/{nonce}", owner.0.0, repo.0.0)
}

/// The chain, repository, artifact, and ticket in the path after `artifact-zip/`.
pub fn parse_zip_path(path: &str) -> Option<(ChainId, RepoId, i64, &str)> {
    let parts: Vec<&str> = path.split('/').collect();
    let [o, s, r, id, nonce] = parts.as_slice() else { return None };
    let positive = |v: &str| v.parse::<i64>().ok().filter(|n| *n > 0);
    if nonce.len() != 32 || !nonce.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return None;
    }
    Some((
        ChainId::Actions(graftyard_types::AccountId(DbId(positive(o)?)), s.parse::<u8>().ok()?),
        RepoId(DbId(positive(r)?)),
        positive(id)?,
        nonce,
    ))
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

    #[test]
    fn rest_retention_defaults_caps_and_sets_expiry() {
        for (asked, days) in [(None, 90), (Some(0), 90), (Some(-3), 90), (Some(1), 1), (Some(30), 30), (Some(400), 90)]
        {
            assert_eq!(retention_days(asked), days, "{asked:?}");
        }
        let created = 1_234_567;
        assert_eq!(ArtifactRow::expires_at(created, 1) - created, DAY_MS);
    }

    #[test]
    fn rest_artifact_ids_are_stable_scoped_and_safe() {
        let base = artifact_id(CHAIN, c().repo, c().run, "art");
        assert_eq!(base, artifact_id(CHAIN, c().repo, c().run, "art"));
        let other_chain = ChainId::Actions(AccountId(DbId(7_100_001)), 12);
        let others = [
            artifact_id(CHAIN, c().repo, RunId(DbId(42)), "art"),
            artifact_id(CHAIN, RepoId(DbId(65_001)), c().run, "art"),
            artifact_id(CHAIN, c().repo, c().run, "other"),
            artifact_id(other_chain, c().repo, c().run, "art"),
        ];
        assert!((1..(1_i64 << 53)).contains(&base));
        for other in others {
            assert_ne!(base, other);
            assert!((1..(1_i64 << 53)).contains(&other), "{other}");
        }
    }

    fn rest_row(id: i64, name: &str, run: i64, created: u64) -> ArtifactRow {
        ArtifactRow {
            id,
            name: name.into(),
            size: 123,
            run: RunId(DbId(run)),
            created,
            updated: created,
            expires: ArtifactRow::expires_at(created, 1),
            head_sha: "abc123".into(),
            head_branch: None,
        }
    }

    #[test]
    fn rest_artifact_json_matches_ghes() {
        let row = ArtifactRow { id: 1, created: 0, updated: 0, expires: DAY_MS, ..rest_row(1, "art", 41, 0) };
        let ctx = RestCtx { api: "https://h/api/v3", full_name: "o/r", repo: RepoId(DbId(65_000)) };
        let before = rest_json(&ctx, &row, DAY_MS - 1);
        assert_eq!(before.as_object().unwrap().len(), 11);
        assert_eq!(before["node_id"], "MDg6QXJ0aWZhY3Qx");
        assert_eq!(before["name"], "art");
        assert_eq!(before["size_in_bytes"], 123);
        assert_eq!(before["url"], "https://h/api/v3/repos/o/r/actions/artifacts/1");
        assert_eq!(before["archive_download_url"], "https://h/api/v3/repos/o/r/actions/artifacts/1/zip");
        assert_eq!(before["expired"], false);
        assert_eq!(rest_json(&ctx, &row, DAY_MS)["expired"], true);
        assert_eq!(before["created_at"], "1970-01-01T00:00:00Z");
        assert_eq!(before["expires_at"], "1970-01-02T00:00:00Z");
        assert_eq!(before["updated_at"], "1970-01-01T00:00:00Z");
        assert_eq!(before["workflow_run"]["id"], 41);
        assert_eq!(before["workflow_run"]["repository_id"], 65_000);
        assert_eq!(before["workflow_run"]["head_repository_id"], 65_000);
        assert_eq!(before["workflow_run"]["head_branch"], Value::Null);
        assert_eq!(before["workflow_run"]["head_sha"], "abc123");
    }

    #[test]
    fn rest_pages_filter_sort_and_count_before_slicing() {
        let rows = || {
            vec![
                rest_row(1, "art", 41, 100),
                rest_row(2, "other", 41, 400),
                rest_row(3, "art", 42, 300),
                rest_row(4, "art", 41, 300),
                rest_row(5, "art", 41, 200),
            ]
        };
        let ids = |rows: &[ArtifactRow]| rows.iter().map(|r| r.id).collect::<Vec<_>>();
        for (page, expected, more) in [(1, vec![2, 4], true), (2, vec![3, 5], true), (3, vec![1], false)] {
            let (total, got, has_more) = page_rows(rows(), None, None, graftyard_wire::Page { per_page: 2, page });
            assert_eq!(total, 5);
            assert_eq!(ids(&got), expected, "page {page}");
            assert_eq!(has_more, more, "page {page}");
        }
        let p = graftyard_wire::Page { per_page: 2, page: 1 };
        let (total, got, more) = page_rows(rows(), Some("art"), None, p);
        assert_eq!((total, ids(&got), more), (4, vec![4, 3], true));
        let (total, got, more) = page_rows(rows(), None, Some(RunId(DbId(42))), p);
        assert_eq!((total, ids(&got), more), (1, vec![3], false));
        let (total, got, more) = page_rows(rows(), Some("art"), Some(RunId(DbId(41))), p);
        assert_eq!((total, ids(&got), more), (3, vec![4, 5], true));
    }

    #[test]
    fn every_artifact_key_maps_to_only_its_name() {
        for (key, name) in [
            ("m/art", "art"),
            ("c/art2", "art2"),
            ("i/art/x/00000000000000000000", "art"),
            ("u/9/art2/x", "art2"),
            ("f/9/5/art/x/00000000000000000000", "art"),
        ] {
            assert_eq!(of_artifact(key), Some(name), "{key}");
        }
        for bad in ["m/", "m", "m/art/x", "c/", "i/art", "i/art/", "u/9/art", "f/9/5/art", "z/art"] {
            assert_eq!(of_artifact(bad), None, "{bad}");
        }
        assert_eq!(zip_path("art/dir/f.bin", "art"), Some("dir/f.bin"));
        assert_eq!(zip_path("other/dir/f.bin", "art"), None);
        assert_eq!(zip_path("art", "art"), None);
    }

    #[test]
    fn rest_create_and_index_keys_stay_in_the_repository() {
        let container = c();
        assert_eq!(container.create_key("art"), format!("{}c/art", container.prefix()));
        assert_eq!(index_key(CHAIN, container.repo, 9), "actions/artifacts/actions:7100001.11/65000/a/9");
        assert_eq!(index_prefix(CHAIN, container.repo), "actions/artifacts/actions:7100001.11/65000/a/");
    }

    /// BF-75's expiry line: a `retention-days: 1` artifact expires one day after its create POST, not after its
    /// finalize and not after 90 days; one with no retention asked expires after 90.
    #[test]
    fn a_finalized_row_expires_by_its_create_records_retention() {
        let run = crate::shard::WorkflowRun {
            id: c().run,
            repo: c().repo,
            workflow: "ci.yml".into(),
            event: "push".into(),
            head_sha: [0xab; 20],
            head_ref: "refs/heads/main".into(),
            trigger: (ChainId::Repo(c().repo), graftyard_types::Index(1)),
            attempt: 1,
            concurrency: None,
            fork: false,
        };
        let created = 1_000_000;
        let finalized = created + 5 * 60_000;
        let short = finalized_row(&c(), "a", 40, Some(&create_record(Some(1), created)), &run, finalized);
        assert_eq!((short.created, short.updated, short.expires), (created, finalized, created + DAY_MS));
        assert_eq!(short.id, artifact_id(CHAIN, c().repo, c().run, "a"));
        assert_eq!(short.head_sha, "ab".repeat(20));
        assert_eq!(short.head_branch.as_deref(), Some("main"));
        let long = finalized_row(&c(), "b", 40, Some(&create_record(None, created)), &run, finalized);
        assert_eq!(long.expires, created + 90 * DAY_MS);
        // A finalize whose create record is gone keeps 90 days from the finalize.
        assert_eq!(finalized_row(&c(), "b", 40, None, &run, finalized).expires, finalized + 90 * DAY_MS);
        let tag = crate::shard::WorkflowRun { head_ref: "refs/tags/v1".into(), ..run };
        assert_eq!(finalized_row(&c(), "a", 1, None, &tag, finalized).head_branch, None);
    }

    #[test]
    fn a_zip_ticket_admits_only_its_artifact_until_it_expires() {
        let t = json!({ "id": 41, "exp": 100 });
        assert!(ticket_admits(&t, 41, Timestamp(99_999)));
        assert!(!ticket_admits(&t, 41, Timestamp(100_000)), "expired at exp");
        assert!(!ticket_admits(&t, 42, Timestamp(1)), "another artifact");
        assert!(!ticket_admits(&json!({ "id": 41 }), 41, Timestamp(1)), "no expiry");
        let n = ticket_name(&[0xab; 16]);
        assert_eq!(n.len(), 32);
        assert_eq!(ticket_key(CHAIN, c().repo, &n), format!("{}z/{n}", repo_prefix(CHAIN, c().repo)));
    }

    #[test]
    fn zip_urls_parse_back_and_bad_paths_do_not() {
        let repo = c().repo;
        let n = ticket_name(&[0x0f; 16]);
        let url = zip_url("https://h/_services/", CHAIN, repo, 41, &n);
        assert_eq!(url, format!("https://h/_services/artifact-zip/7100001/11/65000/41/{n}"));
        let path = url.strip_prefix("https://h/_services/artifact-zip/").unwrap();
        assert_eq!(parse_zip_path(path), Some((CHAIN, repo, 41, n.as_str())));
        // Shard 0 is a shard (`actions_shard` is a hash mod the shard count).
        let zero = ChainId::Actions(AccountId(DbId(7100001)), 0);
        assert_eq!(parse_zip_path(&format!("7100001/0/65000/41/{n}")), Some((zero, repo, 41, n.as_str())));
        for bad in [
            format!("1/2/3/{n}"),
            format!("1/2/3/4/{n}/x"),
            format!("0/2/3/4/{n}"),
            format!("1/x/3/4/{n}"),
            format!("1/2/3/-4/{n}"),
            "1/2/3/4/short".to_owned(),
            format!("1/2/3/4/{}", n.to_uppercase()),
        ] {
            assert_eq!(parse_zip_path(&bad), None, "{bad}");
        }
    }
}
