//! `actions-artifacts` v4 (`design/SHARED.md`, "Actions", Artifacts, T1; `decisions/DECISIONS.md`, "`actions-artifacts`",
//! the v4 amendment): the pure half of the Twirp `ArtifactService` that `@actions/artifact` 2.x speaks where `isGhes()`
//! is false, which on Graftyard is only the `graftyard-actions` forks. The routes are in `workers/edge/src/artifacts.rs`.
//!
//! A v4 artifact is one zip the client uploads through the Azure block dialect: each 8 MiB buffer a Put Block, then one
//! Put Block List. Its objects sit under the run prefix of `crate::artifacts` beside v3's:
//! - `v/<name>/<upload>/<block>`: one staged block, `<block>` the hex of its base64 id;
//! - `w/<name>`: the committed blob (`Committed`), the blocks of one upload in the list's order;
//! - `m/<name>` and the REST row `a/<id>`, written by the finalize exactly as v3's are, so BF-75's lists, zip, and
//!   delete serve a v4 artifact unchanged.

use graftyard_types::{ChainId, DbId, RepoId, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::artifacts::{ArtifactRow, Container, RETENTION_DAYS_MAX, repo_prefix};
use crate::ids::{JobId, RunId};
use crate::runsvc::job_guid;

/// The path the client posts each call to, at the origin of `ACTIONS_RESULTS_URL` (`ArtifactHttpClient.request`).
pub const PREFIX: &str = "/twirp/github.actions.results.api.v1.ArtifactService/";
/// A block id's base64 characters at most: Azure's ids are at most 64 bytes before encoding [doc: Azure Storage,
/// "Put Block", `blockid`].
const BLOCK_ID_MAX: usize = 88;
const DAY_MS: u64 = 86_400_000;

/// The run and job backend ids the client reads from the job token's `scp` claim (`getBackendIdsFromToken`) and sends
/// back in every call: the run's plan GUID and the job's GUID, as the job message names them.
pub fn backend_ids(run: RunId, job: JobId) -> (String, String) {
    (job_guid(2, run.0.0), job_guid(1, job.0.0))
}

/// The job token's `scp` claim: `Actions.Results:<run backend id>:<job backend id>`.
pub fn results_scope(run: RunId, job: JobId) -> String {
    let (r, j) = backend_ids(run, job);
    format!("Actions.Results:{r}:{j}")
}

/// A Twirp error: `{code, msg}` with its HTTP status. The client shows `msg` and retries only 429 and 5xx.
pub fn error(status: u16, msg: &str) -> (u16, Value) {
    let code = match status {
        400 => "invalid_argument",
        401 => "unauthenticated",
        403 => "permission_denied",
        404 => "not_found",
        409 => "already_exists",
        _ => "unavailable",
    };
    (status, json!({ "code": code, "msg": msg }))
}

/// The retention a create asks for: its `expires_at` (an RFC 3339 `Timestamp`, which the client sets to now plus
/// `retention-days`) as whole days from `now`, to the nearest day; `None` without one, which the row takes as 90.
pub fn retention_asked(expires_at: Option<&str>, now_ms: u64) -> Option<i64> {
    let at = graftyard_wire::time::parse_iso(expires_at?)?;
    let ahead = at.0.saturating_sub(now_ms);
    Some(((ahead + DAY_MS / 2) / DAY_MS).min(RETENTION_DAYS_MAX) as i64)
}

/// A request's `workflow_run_backend_id` names the token's own run, or the call is refused.
pub fn own_run(body: &Value, run: RunId) -> bool {
    body["workflow_run_backend_id"].as_str() == Some(backend_ids(run, JobId(DbId(0))).0.as_str())
}

/// A finalized row as `ListArtifacts` answers it, its int64s as strings and its `digest` a bare string, as protobuf's
/// JSON writes `Int64Value` and `StringValue`.
pub fn list_json(row: &ArtifactRow, job: JobId) -> Value {
    let (r, j) = backend_ids(row.run, job);
    let mut v = json!({
        "workflow_run_backend_id": r,
        "workflow_job_run_backend_id": j,
        "database_id": row.id.to_string(),
        "name": row.name,
        "size": row.size.to_string(),
        "created_at": graftyard_wire::time::iso(Timestamp(row.created)),
    });
    if let Some(d) = &row.digest {
        v["digest"] = json!(d);
    }
    v
}

/// The run's unexpired rows that `ListArtifacts`' filters keep, newest first. `id_filter` is an `Int64Value`, a
/// string or a number in JSON.
pub fn list_rows(rows: Vec<ArtifactRow>, run: RunId, body: &Value, now_ms: u64) -> Vec<ArtifactRow> {
    let name = body["name_filter"].as_str();
    let id = match &body["id_filter"] {
        Value::String(s) => s.parse::<i64>().ok(),
        v => v.as_i64(),
    };
    let mut out: Vec<ArtifactRow> = rows
        .into_iter()
        .filter(|r| {
            r.run == run && !r.expired(now_ms) && name.is_none_or(|n| r.name == n) && id.is_none_or(|i| r.id == i)
        })
        .collect();
    out.sort_by(|a, b| b.created.cmp(&a.created).then_with(|| b.id.cmp(&a.id)));
    out
}

// ---- the block shim

/// A block id as a key part: the hex of its base64 characters, a query's `+` read back from the space a form decode
/// makes of it. `None` for an empty or overlong id, or one with characters base64 does not use.
pub fn block_part(id: &str) -> Option<String> {
    let id = id.replace(' ', "+");
    let ok = |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'=' | b'-' | b'_');
    (!id.is_empty() && id.len() <= BLOCK_ID_MAX && id.bytes().all(ok))
        .then(|| id.bytes().map(|b| format!("{b:02x}")).collect())
}

/// The block ids a Put Block List body names, in order: every `<Latest>`, `<Committed>`, and `<Uncommitted>`, which
/// here all mean a block this upload staged. `None` for a body that is not a block list.
pub fn block_list(xml: &str) -> Option<Vec<String>> {
    let start = xml.find("<BlockList>")? + "<BlockList>".len();
    let body = &xml[start..xml.find("</BlockList>")?];
    let mut out = Vec::new();
    let mut rest = body.trim();
    while !rest.is_empty() {
        let open = rest.strip_prefix('<')?;
        let (tag, after) = open.split_once('>')?;
        if !matches!(tag, "Latest" | "Committed" | "Uncommitted") {
            return None;
        }
        let close = format!("</{tag}>");
        let end = after.find(&close)?;
        out.push(after[..end].trim().to_owned());
        rest = after[end + close.len()..].trim_start();
    }
    Some(out)
}

/// The committed blob of one artifact: the upload whose blocks it is, those blocks' key parts and sizes in order, and
/// their total.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Committed {
    pub upload: u64,
    pub blocks: Vec<(String, u64)>,
    pub size: u64,
}

/// The blob a block list commits, given the blocks `upload` staged (key part, size): each id it names must be one of
/// them, else why not.
pub fn commit(upload: u64, staged: &[(String, u64)], list: &[String]) -> Result<Committed, String> {
    let mut blocks = Vec::with_capacity(list.len());
    for id in list {
        let part = block_part(id).ok_or_else(|| format!("the block id {id:?} is not valid"))?;
        let size = staged
            .iter()
            .find(|(p, _)| *p == part)
            .map(|(_, n)| *n)
            .ok_or_else(|| format!("the block {id} was not staged by this upload"))?;
        blocks.push((part, size));
    }
    let size = blocks.iter().map(|(_, n)| n).sum();
    Ok(Committed { upload, blocks, size })
}

impl Container {
    /// Every staged block of artifact `name`, every upload's.
    pub fn blocks(&self, name: &str) -> String {
        format!("{}v/{name}/", self.prefix())
    }

    pub fn block_prefix(&self, name: &str, upload: u64) -> String {
        format!("{}{upload}/", self.blocks(name))
    }

    pub fn block_key(&self, name: &str, upload: u64, part: &str) -> String {
        format!("{}{part}", self.block_prefix(name, upload))
    }

    /// The committed blob's record.
    pub fn blob_key(&self, name: &str) -> String {
        format!("{}w/{name}", self.prefix())
    }
}

// ---- the signed upload URL

/// What an upload ticket admits: one job's upload of one artifact name in its run, until `exp` (seconds). It sits
/// beside the zip tickets (`crate::artifacts::ticket_key`), so a repository delete drops it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UploadTicket {
    pub run: RunId,
    pub job: JobId,
    pub name: String,
    pub upload: u64,
    pub exp: u64,
}

impl UploadTicket {
    pub fn admits(&self, now: Timestamp) -> bool {
        self.exp > now.0 / 1000
    }
}

/// The signed upload URL: `artifact-blob/<owner>/<shard>/<repo>/<ticket>` under the run service. Its path names no
/// container the Azure client could split (`getBlobAndContainerNamesFromUrl` keeps the whole URL), and the client
/// appends `comp` and `blockid` as its query.
pub fn upload_url(services: &str, chain: ChainId, repo: RepoId, nonce: &str) -> String {
    let ChainId::Actions(owner, shard) = chain else { unreachable!("an Actions shard") };
    format!("{services}artifact-blob/{}/{shard}/{}/{nonce}", owner.0.0, repo.0.0)
}

/// The chain, repository, and ticket in the path after `artifact-blob/`.
pub fn parse_upload_path(path: &str) -> Option<(ChainId, RepoId, &str)> {
    let parts: Vec<&str> = path.split('/').collect();
    let [o, s, r, nonce] = parts.as_slice() else { return None };
    let positive = |v: &str| v.parse::<i64>().ok().filter(|n| *n > 0);
    if nonce.len() != 32 || !nonce.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return None;
    }
    Some((
        ChainId::Actions(graftyard_types::AccountId(DbId(positive(o)?)), s.parse::<u8>().ok()?),
        RepoId(DbId(positive(r)?)),
        nonce,
    ))
}

/// Where an upload ticket lives: the repository's ticket prefix, as the zip tickets.
pub fn upload_ticket_key(chain: ChainId, repo: RepoId, nonce: &str) -> String {
    format!("{}y/{nonce}", repo_prefix(chain, repo))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifacts::of_artifact;
    use graftyard_types::AccountId;

    const CHAIN: ChainId = ChainId::Actions(AccountId(DbId(7100001)), 11);

    fn row(id: i64, name: &str, run: i64, created: u64) -> ArtifactRow {
        ArtifactRow {
            id,
            name: name.into(),
            size: 20_006_340,
            run: RunId(DbId(run)),
            created,
            updated: created,
            expires: created + DAY_MS,
            head_sha: "ab".repeat(20),
            head_branch: Some("main".into()),
            digest: Some("sha256:837b".into()),
        }
    }

    /// The scope the client parses (`getBackendIdsFromToken`: split on spaces, then `:`, three parts) names the plan and
    /// job GUIDs the job message names, and the call's run id must be that plan's.
    #[test]
    fn the_scope_names_the_job_messages_run_and_job() {
        let (run, job) = (RunId(DbId(3)), JobId(DbId(9)));
        let scp = results_scope(run, job);
        let parts: Vec<&str> = scp.split(':').collect();
        assert_eq!(
            parts,
            ["Actions.Results", "00000000-0000-4002-8000-000000000003", "00000000-0000-4001-8000-000000000009"]
        );
        assert!(own_run(&json!({ "workflow_run_backend_id": parts[1] }), run));
        assert!(!own_run(&json!({ "workflow_run_backend_id": parts[2] }), run), "the job's GUID is not the run's");
        assert!(!own_run(&json!({ "workflow_run_backend_id": job_guid(2, 4) }), run), "another run");
        assert!(!own_run(&json!({}), run));
    }

    /// The client sets `expires_at` to now plus `retention-days`, a few ms before the create reaches the edge.
    #[test]
    fn retention_is_the_expiry_in_days_to_the_nearest() {
        let now = 1_764_000_000_000;
        let iso = |ms: u64| graftyard_wire::time::iso(Timestamp(ms));
        assert_eq!(retention_asked(Some(&iso(now + 3 * DAY_MS - 2_000)), now), Some(3));
        assert_eq!(retention_asked(Some(&iso(now + 3 * DAY_MS + 2_000)), now), Some(3));
        // The client's own spelling, with milliseconds: 2026-11-27T00:00:00.971Z is 1,795,737,600,971 ms.
        assert_eq!(retention_asked(Some("2026-11-27T00:00:00.971Z"), 1_795_737_600_971 - DAY_MS + 40), Some(1));
        assert_eq!(retention_asked(Some(&iso(now + DAY_MS / 3)), now), Some(0), "under half a day rounds to 0");
        assert_eq!(retention_asked(Some(&iso(now + 400 * DAY_MS)), now), Some(90));
        assert_eq!(retention_asked(None, now), None);
        assert_eq!(retention_asked(Some("not a time"), now), None);
    }

    #[test]
    fn list_filters_the_runs_unexpired_rows_newest_first() {
        let rows = vec![row(1, "a", 41, 10), row(2, "b", 41, 30), row(3, "a", 42, 20), row(4, "c", 41, 20)];
        let ids = |b: Value, now: u64| -> Vec<i64> {
            list_rows(rows.clone(), RunId(DbId(41)), &b, now).iter().map(|r| r.id).collect()
        };
        assert_eq!(ids(json!({}), 0), [2, 4, 1], "only run 41, newest first");
        assert_eq!(ids(json!({ "name_filter": "a" }), 0), [1]);
        assert_eq!(ids(json!({ "id_filter": "4" }), 0), [4]);
        assert_eq!(ids(json!({ "id_filter": 2 }), 0), [2]);
        assert_eq!(ids(json!({ "id_filter": "3" }), 0), Vec::<i64>::new(), "run 42's id from run 41");
        assert_eq!(ids(json!({}), 10 + DAY_MS), [2, 4], "row 1 expired");
        let v = list_json(&row(7, "art", 41, 0), JobId(DbId(9)));
        assert_eq!(v["database_id"], "7");
        assert_eq!(v["size"], "20006340");
        assert_eq!(v["digest"], "sha256:837b", "a StringValue is a bare string in protobuf JSON");
        assert_eq!(v["workflow_run_backend_id"], job_guid(2, 41));
        assert_eq!(v["created_at"], "1970-01-01T00:00:00Z");
        assert!(
            list_json(&ArtifactRow { digest: None, ..row(7, "art", 41, 0) }, JobId(DbId(9))).get("digest").is_none()
        );
    }

    /// The body `commitBlockList` sends, in order, and the ids `stageBlock` sent, a `+` read back from a form decode.
    #[test]
    fn a_block_list_commits_only_this_uploads_staged_blocks_in_its_order() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><BlockList><Latest>QUFB</Latest><Latest>Qk+C</Latest><Latest>Q0ND</Latest></BlockList>"#;
        let list = block_list(xml).unwrap();
        assert_eq!(list, ["QUFB", "Qk+C", "Q0ND"]);
        let p = |s: &str| block_part(s).unwrap();
        assert_eq!(p("Qk C"), p("Qk+C"), "a query's + decoded as a space is the same block");
        assert_ne!(p("QUFB"), p("Q0ND"));
        // Staged out of order, as five concurrent Put Blocks land.
        let staged = vec![(p("Q0ND"), 3), (p("QUFB"), 8), (p("Qk+C"), 8), (p("RERE"), 8)];
        let c = commit(5, &staged, &list).unwrap();
        assert_eq!(c.blocks, vec![(p("QUFB"), 8), (p("Qk+C"), 8), (p("Q0ND"), 3)], "the list's order, not staging's");
        assert_eq!((c.upload, c.size), (5, 19), "RERE staged but not listed is not in the blob");
        assert!(commit(5, &staged[..2], &list).is_err(), "a listed block this upload never staged");
        assert!(block_list("<BlockList><Latest>a</Latest><Bad>b</Bad></BlockList>").is_none());
        assert!(block_list("<NotAList/>").is_none());
        assert_eq!(block_list("<BlockList></BlockList>"), Some(vec![]));
        for bad in ["", "a<b", &"A".repeat(BLOCK_ID_MAX + 1)] {
            assert_eq!(block_part(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_upload_url_parses_back_and_its_keys_belong_to_the_artifact() {
        let nonce = "0123456789abcdef0123456789abcdef";
        let url = upload_url("https://h/_services/", CHAIN, RepoId(DbId(65_000)), nonce);
        assert_eq!(url, format!("https://h/_services/artifact-blob/7100001/11/65000/{nonce}"));
        let tail = url.strip_prefix("https://h/_services/artifact-blob/").unwrap();
        assert_eq!(parse_upload_path(tail), Some((CHAIN, RepoId(DbId(65_000)), nonce)));
        for bad in [&nonce[1..], "0123456789ABCDEF0123456789abcdef", "x"] {
            assert_eq!(parse_upload_path(&format!("7100001/11/65000/{bad}")), None, "{bad}");
        }
        assert_eq!(parse_upload_path(&format!("7100001/11/65000/41/{nonce}")), None);
        let t = UploadTicket { run: RunId(DbId(41)), job: JobId(DbId(9)), name: "art".into(), upload: 5, exp: 100 };
        assert!(t.admits(Timestamp(99_999)) && !t.admits(Timestamp(100_000)));
        // A delete of `art` drops its blocks and blob record, and no other artifact's.
        let ct = Container { chain: CHAIN, repo: RepoId(DbId(65_000)), run: RunId(DbId(41)) };
        let rel = |k: String| k.strip_prefix(&ct.prefix()).unwrap().to_owned();
        assert_eq!(of_artifact(&rel(ct.block_key("art", 5, &block_part("QUFB").unwrap()))), Some("art"));
        assert_eq!(of_artifact(&rel(ct.blob_key("art"))), Some("art"));
        assert_eq!(of_artifact(&rel(ct.blob_key("art2"))), Some("art2"));
        assert!(!upload_ticket_key(CHAIN, ct.repo, nonce).starts_with(&ct.prefix()), "tickets are not the run's");
    }
}
