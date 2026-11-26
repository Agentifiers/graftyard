//! The launch service's pure half (`design/SHARED.md`, "Actions", Runner protocol; `decisions/DECISIONS.md`, "M4-WP3
//! slice A: the launch service"): which job a `runnerresolve` call is for, what it asks for, how one `uses:` version
//! resolves against a repository's refs, and the reply an unmodified `actions/runner` 2.338.2 parses. The hosts (the
//! edge's `launch` route and `examples/runsvc_host.rs`) read the repositories; everything here is a function of its
//! inputs.
//!
//! - **The route.** The runner joins `/actions/build/{plan}/jobs/{job}/runnerresolve/actions` onto
//!   `system.github.launch_endpoint` as an absolute path, so the call lands at the install host's root whatever path
//!   the endpoint has [measured from source: `LaunchHttpClient.cs:42`, `new Uri(m_launchServiceUrl, "/actions/...")`].
//! - **The reply.** 200 `{"actions": {"<action>@<version>": {...}}}`, keyed by the request's text so the runner's
//!   ordinal lookup finds it (`ActionManager.cs:1589-1617`); 422 `{"errors": {"<key>": {"message"}}}`, which the
//!   runner joins with ". " and does not retry (`LaunchHttpClient.cs:52-66`, `LaunchContracts.cs:89-106`,
//!   `ActionManager.cs:1078-1100`); 503 for a read that failed, which it retries.

use graftyard_git::refs::{ObjectId, RefName};
use serde_json::{Map, Value, json};

use crate::ids::JobId;
use crate::runsvc::{Grant, job_guid, job_of_guid};

/// The job a `POST /actions/build/{plan}/jobs/{job}/runnerresolve/actions` is for: only when the bearer token's grant
/// is that job's. Any other path or grant is not a launch call, and the host answers it as it did before.
pub fn job_of_path(path: &str, grant: Option<Grant>) -> Option<JobId> {
    let rest = path.strip_prefix("/actions/build/")?.strip_suffix("/runnerresolve/actions")?;
    let (plan, job) = rest.split_once("/jobs/")?;
    let job = job_of_guid(job)?;
    let guid = plan.len() == 36
        && plan
            .bytes()
            .enumerate()
            .all(|(i, b)| if [8, 13, 18, 23].contains(&i) { b == b'-' } else { b.is_ascii_hexdigit() });
    match grant {
        Some(Grant::Job { job: j, .. }) if guid && j == job => Some(job),
        _ => None,
    }
}

/// One action the runner asks for: `name` is `owner/repo` as the workflow wrote it, `key` is `{name}@{version}`.
/// `resolved` is the repository whose bytes the reply names instead, when an org's substitution policy chose a
/// `graftyard-actions` fork (`crate::pin_map::redirect`); the reply keeps `name` and `version` as written, so the
/// runner files the download under the action it asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionRef {
    pub key: String,
    pub name: String,
    pub owner: String,
    pub repo: String,
    pub version: String,
    pub resolved: Option<String>,
}

/// The actions a request names, once each by key, in order; `None` for a body that is not the runner's
/// `ActionReferenceRequestList` (`LaunchContracts.cs:8-27`). A name that is not two non-empty segments keeps an empty
/// owner, which no repository has.
pub fn requested(body: &Value) -> Option<Vec<ActionRef>> {
    let mut out: Vec<ActionRef> = Vec::new();
    for a in body.get("actions")?.as_array()? {
        let name = a.get("action")?.as_str()?.to_owned();
        let version = a.get("version")?.as_str()?.to_owned();
        let key = format!("{name}@{version}");
        if out.iter().any(|x| x.key == key) {
            continue;
        }
        let (owner, repo) = match name.split_once('/') {
            Some((o, r)) if !o.is_empty() && !r.is_empty() && !r.contains('/') => (o.to_owned(), r.to_owned()),
            _ => (String::new(), String::new()),
        };
        out.push(ActionRef { key, name, owner, repo, version, resolved: None });
    }
    Some(out)
}

/// What the host found for one action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Found {
    /// The commit, and the archive URLs the install serves itself (`archive_urls`).
    Commit {
        sha: ObjectId,
        tar_url: String,
        zip_url: String,
    },
    /// No such repository on the install, or one the job may not read: the same answer, so a private repository's
    /// name is not confirmed.
    NoRepo,
    NoVersion,
    /// An abbreviated commit SHA, which GitHub refuses too [doc: "Using pre-written building blocks in your
    /// workflow", full-length commit SHA].
    Short,
    /// A read that did not answer: the call is answered 503 and the runner retries it.
    Failed(String),
}

/// How a version resolved: through a ref, whose commit the repository holds, or as a full SHA, which the host must
/// still find in the repository before it answers (`Found::NoVersion` when it does not).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resolved {
    Ref(ObjectId),
    Sha(ObjectId),
}

/// `version` in a repository whose ref `ref_value` reads: a full SHA (either case), else the full ref, the tag, then
/// the branch of that name, git's order. The edge's narrow read records `graftyard_api::release::resolve`'s lookups,
/// which are these in this order, so every name read here is one the member recorded.
pub fn resolve(version: &str, ref_value: impl Fn(&RefName) -> Option<ObjectId>) -> Result<Resolved, Found> {
    if let Some(sha) = ObjectId::parse_hex(version.to_ascii_lowercase().as_bytes()) {
        return Ok(Resolved::Sha(sha));
    }
    let named = ["", "refs/tags/", "refs/heads/"]
        .iter()
        .filter_map(|p| RefName::parse(format!("{p}{version}").as_bytes()).ok())
        .find_map(|n| ref_value(&n));
    if let Some(sha) = named {
        return Ok(Resolved::Ref(sha));
    }
    let short = (4..40).contains(&version.len()) && version.bytes().all(|b| b.is_ascii_hexdigit());
    Err(if short { Found::Short } else { Found::NoVersion })
}

/// A commit's archives under `codeload`, the repository's codeload base (`graftyard_wire::Host::codeload`): served by
/// the install with no redirect, with one top directory, which the runner unwraps (`ActionManager.cs:1366-1374`).
pub fn archive_urls(codeload: &str, sha: ObjectId) -> (String, String) {
    let h = sha.hex();
    (format!("{codeload}/legacy.tar.gz/{h}"), format!("{codeload}/legacy.zip/{h}"))
}

/// The reply to a resolve call, as `(status, body)`.
pub fn reply(found: &[(ActionRef, Found)]) -> (u16, Value) {
    if let Some(e) = found.iter().find_map(|(_, f)| if let Found::Failed(e) = f { Some(e) } else { None }) {
        return (503, json!({ "message": format!("the action repositories did not answer; retry ({e})") }));
    }
    let (mut actions, mut errors) = (Map::new(), Map::new());
    for (a, f) in found {
        let why = match f {
            Found::Commit { sha, tar_url, zip_url } => {
                // `name` and `version` as written (the key's halves), which the runner files the download under and
                // later loads from (`ActionManager.cs:1193`, `:1420`); `resolved_name` the repository whose bytes these
                // are, a fork when substituted.
                let resolved = a.resolved.as_deref().unwrap_or(&a.name);
                let version = &a.key[a.name.len() + 1..];
                let info = json!({ "name": a.name, "resolved_name": resolved, "resolved_sha": sha.hex(),
                                   "tar_url": tar_url, "zip_url": zip_url, "version": version });
                actions.insert(a.key.clone(), info);
                continue;
            }
            Found::NoRepo => "repository not found".to_owned(),
            Found::NoVersion => format!("unable to find version '{}'", a.version),
            Found::Short => format!(
                "the provided ref '{}' is the shortened version of a commit SHA, which is not supported. Please use \
                 the full commit SHA instead",
                a.version
            ),
            Found::Failed(_) => unreachable!("answered 503 above"),
        };
        errors.insert(a.key.clone(), json!({ "message": format!("Unable to resolve action '{}', {why}", a.key) }));
    }
    if errors.is_empty() { (200, json!({ "actions": actions })) } else { (422, json!({ "errors": errors })) }
}

/// The path a runner posts to for job `job` of run `run_id`, as `job_message` names them.
pub fn path_for(run_id: i64, job: JobId) -> String {
    format!("/actions/build/{}/jobs/{}/runnerresolve/actions", job_guid(2, run_id), job_guid(1, job.0.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use graftyard_types::DbId;
    use std::collections::BTreeMap;

    fn oid(b: u8) -> ObjectId {
        ObjectId([b; 20])
    }

    fn refs(pairs: &[(&str, u8)]) -> impl Fn(&RefName) -> Option<ObjectId> {
        let m: BTreeMap<RefName, ObjectId> =
            pairs.iter().map(|(n, b)| (RefName::parse(n.as_bytes()).unwrap(), oid(*b))).collect();
        move |n| m.get(n).copied()
    }

    #[test]
    fn a_version_resolves_as_a_full_sha_then_a_tag_then_a_branch() {
        // A tag and a branch both named `v1`: the tag wins, as git's order has it; `main` is only a branch.
        let r = refs(&[("refs/tags/v1", 1), ("refs/heads/v1", 2), ("refs/heads/main", 3), ("refs/tags/abc1234", 4)]);
        assert_eq!(resolve("v1", &r), Ok(Resolved::Ref(oid(1))));
        assert_eq!(resolve("main", &r), Ok(Resolved::Ref(oid(3))));
        assert_eq!(resolve("refs/heads/v1", &r), Ok(Resolved::Ref(oid(2))));
        // A full SHA resolves with no ref lookup, in either case, for the host to find in the repository.
        let full = "ab".repeat(20);
        assert_eq!(resolve(&full, &r), Ok(Resolved::Sha(oid(0xab))));
        assert_eq!(resolve(&full.to_ascii_uppercase(), &r), Ok(Resolved::Sha(oid(0xab))));
        // A hex tag name is a tag, not an abbreviated SHA; one with no such ref is refused as abbreviated.
        assert_eq!(resolve("abc1234", &r), Ok(Resolved::Ref(oid(4))));
        assert_eq!(resolve("abc1235", &r), Err(Found::Short));
        assert_eq!(resolve(&full[..39], &r), Err(Found::Short));
        assert_eq!(resolve("v2", &r), Err(Found::NoVersion));
        assert_eq!(resolve("abc", &r), Err(Found::NoVersion), "too short to be a SHA");
    }

    #[test]
    fn the_reply_echoes_the_request_keys_and_refuses_with_the_runners_422_shape() {
        let body = json!({ "actions": [
            { "action": "Mirror/Hello", "version": "v1", "path": "" },
            { "action": "mirror/hello", "version": "main" },
            { "action": "Mirror/Hello", "version": "v1" },
        ]});
        let asked = requested(&body).unwrap();
        assert_eq!(asked.len(), 2, "one entry per key");
        let (tar, zip) = archive_urls("https://h/_codeload/Mirror/Hello", oid(1));
        let found: Vec<_> = asked
            .iter()
            .map(|a| (a.clone(), Found::Commit { sha: oid(1), tar_url: tar.clone(), zip_url: zip.clone() }))
            .collect();
        let (s, v) = reply(&found);
        assert_eq!(s, 200);
        let one = &v["actions"]["mirror/hello@main"];
        assert_eq!(one["resolved_sha"], json!("01".repeat(20)));
        assert_eq!(one["version"], json!("main"));
        assert_eq!(one["name"], json!("mirror/hello"));
        assert_eq!(
            v["actions"]["Mirror/Hello@v1"]["tar_url"],
            json!(format!("https://h/_codeload/Mirror/Hello/legacy.tar.gz/{}", "01".repeat(20)))
        );
        // One unresolved action refuses the whole call with 422, naming only the failed key, in the shape
        // `ActionDownloadResolutionErrorCollection` reads.
        let mut found = found;
        found[1].1 = Found::Short;
        let (s, v) = reply(&found);
        assert_eq!(s, 422);
        assert_eq!(v["errors"].as_object().unwrap().len(), 1);
        let m = v["errors"]["mirror/hello@main"]["message"].as_str().unwrap();
        assert!(
            m.starts_with("Unable to resolve action 'mirror/hello@main', the provided ref 'main' is the shortened")
        );
        found[1].1 = Found::NoRepo;
        assert_eq!(
            reply(&found).1["errors"]["mirror/hello@main"]["message"],
            json!("Unable to resolve action 'mirror/hello@main', repository not found")
        );
        // A read that failed is retried, not refused.
        found[0].1 = Found::Failed("timeout".into());
        assert_eq!(reply(&found).0, 503);
        // A name that is not owner/repo matches no repository.
        let bad = requested(&json!({ "actions": [{ "action": "a/b/c", "version": "v1" }] })).unwrap();
        assert_eq!((bad[0].owner.as_str(), bad[0].repo.as_str()), ("", ""));
        assert_eq!(requested(&json!({ "nope": 1 })), None);
    }

    #[test]
    fn a_launch_call_is_taken_only_with_its_own_jobs_token() {
        let job = JobId(DbId(589_826));
        let p = path_for(0, job);
        let mine = Some(Grant::Job { job, attempt: 1 });
        assert_eq!(job_of_path(&p, mine), Some(job));
        assert_eq!(job_of_path(&p, Some(Grant::Job { job: JobId(DbId(589_827)), attempt: 1 })), None);
        assert_eq!(job_of_path(&p, Some(Grant::Runner { runner: crate::RunnerId(DbId(5)) })), None);
        assert_eq!(job_of_path(&p, None), None);
        assert_eq!(job_of_path(&p.replace("/actions/build/", "/actions/builds/"), mine), None);
        assert_eq!(job_of_path(&format!("{p}/x"), mine), None);
        assert_eq!(job_of_path(&p.replace("00000000-0000-4002", "not-a-guid-0000-4002"), mine), None);
    }
}
