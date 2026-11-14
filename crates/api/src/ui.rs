//! The web app's private `/_t/ui/*` reads that the repository's fold answers (`design/SHARED.md`, "Frontend and
//! native app", the private API): a pull request's own timeline events, its review threads' resolution, and what
//! stands between it and a merge. Each is computed from the same rows GraphQL's `mergeStateStatus` reads
//! (`crate::gql::merge_state`), so the merge box and `gh pr view` agree. Also the GHES shapes the git cache shard's
//! commits and diff are wrapped in. None of these is a GHES 3.22 path, so no GHES reply changes.

use graftyard_git::issues::{CheckConclusion, IssueNumber, PrState};
use graftyard_git::refs::ObjectId;
use serde_json::{Value, json};

use crate::view::Repo;
use crate::wire::{Reply, iso, simple_user};

fn pull<'r>(r: &Repo<'r>, n: &str) -> Option<crate::view::Item> {
    let n = IssueNumber(n.parse().ok()?);
    r.item(n).filter(crate::view::Item::is_pull)
}

/// `GET pulls/{n}/events`: the timeline events of pull request `n` that the issue timeline does not own, from what
/// the fold keeps. A merge or close is its only such event today; the fold keeps no merge method, so it is `null`.
pub fn events(r: &Repo<'_>, n: &str) -> Reply {
    let Some(it) = pull(r, n) else { return Reply::not_found() };
    let (p, head) = it.pull.as_ref().expect("a pull request");
    let base = p.base.display();
    let base = base.strip_prefix("refs/heads/").unwrap_or(&base).to_owned();
    let out = match head.state {
        PrState::Open { .. } => vec![],
        PrState::Merged { at, by, merge_commit, .. } => vec![json!({
            "id": it.row.id.0, "event": "merged", "actor": simple_user(r.host, r.dir, Some(by.0)),
            "created_at": iso(at), "commit_id": merge_commit.hex(), "merge_method": null, "base_ref": base,
        })],
        PrState::Closed { at, .. } => vec![json!({
            "id": it.row.id.0, "event": "closed", "actor": simple_user(r.host, r.dir, it.row.author),
            "created_at": iso(at),
        })],
    };
    Reply::json(200, Value::Array(out))
}

/// `GET pulls/{n}/threads`: each review thread's root comment and whether it is resolved. The fold does not record
/// who resolved it, so `resolved_by` is `null`.
pub fn threads(r: &Repo<'_>, n: &str) -> Reply {
    let Some(it) = pull(r, n) else { return Reply::not_found() };
    let out: Vec<Value> = r
        .review_comments_of(it.n)
        .into_iter()
        .filter(|(_, c)| c.reply_to.is_none())
        .map(|(id, c)| json!({ "root_id": id.0.0, "resolved": c.resolved, "resolved_by": null }))
        .collect();
    Reply::json(200, Value::Array(out))
}

/// `GET pulls/{n}/merge-requirements`, with `behind_by` and whether the head branch still exists from the caller (the
/// git cache shard's compare and the ref rows). Each requirement row is the web app's `Requirement`; `mergeable_state`
/// is REST's lower case of `mergeStateStatus`, `unknown` while the test merge is computed.
pub fn merge_requirements(r: &Repo<'_>, n: &str, behind_by: u32) -> Reply {
    let Some(it) = pull(r, n) else { return Reply::not_found() };
    let (p, head) = it.pull.as_ref().expect("a pull request");
    let ps = r.protections(&p.base);
    let required_checks: Vec<String> =
        ps.iter().filter_map(|x| x.checks.as_ref()).flat_map(|c| c.checks.iter().map(|(n, _)| n.to_string())).collect();
    let required_approvals = ps.iter().filter_map(|x| x.reviews.as_ref().map(|v| v.count)).max().unwrap_or(0);
    let strict = ps.iter().filter_map(|x| x.checks.as_ref()).any(|c| c.strict);
    let head_exists = r.ref_value(&p.head.branch).is_some();
    let open = matches!(head.state, PrState::Open { .. });
    let draft = matches!(head.state, PrState::Open { draft: true });
    let (merge, _) = crate::gql::test_merge(r, &it);
    let state = if !open {
        "unknown"
    } else if draft {
        "draft"
    } else {
        crate::gql::merge_state(r, &it, merge)
    };
    let mut list = Vec::new();
    let row = |key: &str, state: &str, title: String, detail: String| json!({ "key": key, "state": state, "title": title, "detail": detail });
    if draft {
        list.push(row(
            "draft",
            "failing",
            "This pull request is still a work in progress".into(),
            "Draft pull requests cannot be merged. Mark it ready for review first.".into(),
        ));
    }
    match merge {
        Some(false) => list.push(row(
            "conflicts",
            "failing",
            "This branch has conflicts that must be resolved".into(),
            "Resolve the conflicts on the head branch, then push.".into(),
        )),
        None if open => list.push(row(
            "conflicts",
            "pending",
            "Checking for the ability to merge automatically".into(),
            "The test merge of the head into the base is being computed.".into(),
        )),
        _ => {}
    }
    let verdicts = r.verdicts(it.n);
    let approvals = verdicts.iter().filter(|v| v.state == graftyard_git::issues::ReviewState::Approved).count();
    let changes = verdicts.iter().any(|v| v.state == graftyard_git::issues::ReviewState::ChangesRequested);
    let plural = |n: usize, w: &str| format!("{n} {w}{}", if n == 1 { "" } else { "s" });
    list.push(if changes {
        row(
            "reviews",
            "failing",
            "Changes requested".into(),
            "A reviewer asked for changes. Address them and ask for another review.".into(),
        )
    } else if approvals >= usize::from(required_approvals) {
        row(
            "reviews",
            "ok",
            if approvals > 0 { "Changes approved".into() } else { "No review required".into() },
            if approvals > 0 {
                format!("{}.", plural(approvals, "approving review"))
            } else {
                "The base branch does not require approving reviews.".into()
            },
        )
    } else {
        row(
            "reviews",
            "failing",
            "Review required".into(),
            format!(
                "At least {} by a reviewer with write access is required.",
                plural(usize::from(required_approvals), "approving review")
            ),
        )
    });
    let required = r.required(&head.head_sha);
    let concl = |name: &str| required.iter().find(|(n, _)| n == name).and_then(|(_, row)| row.conclusion);
    let failed = required_checks.iter().filter(|c| concl(c).is_some_and(|x| !CheckConclusion::passes(x))).count();
    let pending = required_checks.iter().filter(|c| concl(c).is_none()).count();
    let runs = r.check_runs(&head.head_sha);
    let done = runs.iter().filter(|t| t.rec.conclusion.is_some()).count();
    let passed = runs.iter().filter(|t| t.rec.conclusion.is_some_and(CheckConclusion::passes)).count();
    let summary = format!("{passed} of {} checks passed.", runs.len());
    list.push(if failed > 0 {
        row("checks", "failing", format!("{} failed", plural(failed, "required check")), summary)
    } else if pending > 0 {
        row(
            "checks",
            "pending",
            format!("{} pending", plural(pending, "required check")),
            "Waiting for status to be reported.".into(),
        )
    } else if done < runs.len() {
        row("checks", "pending", "Some checks have not completed".into(), summary)
    } else if passed < runs.len() {
        row("checks", "pending", "Some checks were not successful".into(), summary)
    } else {
        row("checks", "ok", "All checks have passed".into(), summary)
    });
    if behind_by > 0 && merge != Some(false) {
        list.push(row(
            "behind",
            if strict { "failing" } else { "pending" },
            "This branch is out of date with the base branch".into(),
            format!(
                "{} behind. {}",
                plural(behind_by as usize, "commit"),
                if strict { "Update the branch before merging." } else { "Updating is optional." }
            ),
        ));
    }
    Reply::json(
        200,
        json!({
            "mergeable_state": state.to_ascii_lowercase(), "requirements": list, "required_checks": required_checks,
            "required_approvals": required_approvals, "strict": strict, "behind_by": behind_by,
            "head_exists": head_exists,
        }),
    )
}

/// The base tip and head of pull request `n`, which the shard's diff, compare, and commits read.
pub fn pull_shas(r: &Repo<'_>, n: &str) -> Option<(ObjectId, ObjectId)> {
    let it = pull(r, n)?;
    let (p, head) = it.pull.as_ref()?;
    Some(r.pull_pair(p, head))
}

/// `GET pulls/{n}/commits`: the shard's commits (`{commits: [{sha, raw}]}`) as GHES `commit` items, oldest first.
pub fn commits(r: &Repo<'_>, shard: &Value) -> Reply {
    let mut out = Vec::new();
    for c in shard["commits"].as_array().into_iter().flatten() {
        let (Some(sha), Some(raw)) =
            (c["sha"].as_str().and_then(|s| ObjectId::parse_hex(s.as_bytes())), c["raw"].as_str())
        else {
            continue;
        };
        let one = crate::rest::get_commit(r, sha, raw.as_bytes());
        if one.status == 200 {
            out.push(one.value().clone());
        }
    }
    Reply::json(200, Value::Array(out))
}

/// `GET pulls/{n}/files`: the shard's diff files as GHES `diff-entry` items, with the URLs GHES adds.
pub fn files(r: &Repo<'_>, head: ObjectId, shard: &Value) -> Reply {
    let (api, web) = (r.host.api(), r.host.web());
    let (o, name) = (&r.at.owner, &r.at.name);
    let head = head.hex();
    let out: Vec<Value> = shard["files"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|f| {
            let mut f = f.clone();
            let path = f["filename"].as_str().unwrap_or_default().to_owned();
            let enc: String = url_path(&path);
            f["blob_url"] = json!(format!("{web}/{o}/{name}/blob/{head}/{enc}"));
            f["raw_url"] = json!(format!("{web}/{o}/{name}/raw/{head}/{enc}"));
            f["contents_url"] = json!(format!("{api}/repos/{o}/{name}/contents/{enc}?ref={head}"));
            f
        })
        .collect();
    Reply::json(200, Value::Array(out))
}

/// A path with each segment percent-encoded, as GHES writes file URLs.
fn url_path(p: &str) -> String {
    p.split('/')
        .map(|s| {
            s.bytes()
                .map(|b| {
                    if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                        (b as char).to_string()
                    } else {
                        format!("%{b:02X}")
                    }
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_path_keeps_its_slashes_and_escapes_the_rest() {
        assert_eq!(super::url_path("src/a b/ü.rs"), "src/a%20b/%C3%BC.rs");
    }
}
